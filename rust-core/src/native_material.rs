//! Native text-leaf projection of the existing SIC editing materializer.
//! Native carrier metadata stays immutable; all generated text has an
//! authorized leaf destination, and local reorders never cross opaque blocks.
use crate::{editing,error::{CoreError,Result},transparent::NativeContext};
use serde_json::{Value,json};
use std::collections::{BTreeMap,BTreeSet};

fn text<'a>(value:&'a Value,key:&str) -> Result<&'a str> {
    value[key].as_str().ok_or_else(||CoreError::invalid(format!("{key} must be text")))
}
fn parent_group(context:&NativeContext,id:&str,editable:&[String]) -> Result<String> {
    let unit = context.units().iter().find(|unit|unit.unit_id == id).ok_or_else(||CoreError::invalid("native unit missing"))?;
    let path = unit.anchor.as_array().ok_or_else(||CoreError::invalid("native anchor missing"))?;
    if path.len() < 2 || path.last().and_then(Value::as_str) != Some("text") {
        return Err(CoreError::new("native_reorder_unsupported","reorder requires sibling native text blocks",501));
    }
    let index = path[path.len()-2].as_u64().ok_or_else(||CoreError::invalid("native block index missing"))? as usize;
    let parent = &path[..path.len()-2];
    let mut array = context.source_request();
    for segment in parent {
        array = if let Some(key) = segment.as_str() { &array[key] }
            else { &array[segment.as_u64().ok_or_else(||CoreError::invalid("native path segment invalid"))? as usize] };
    }
    let blocks = array.as_array().ok_or_else(||CoreError::invalid("native parent is not an array"))?;
    let kind = blocks.get(index).and_then(|block|block["type"].as_str()).ok_or_else(||CoreError::invalid("native block kind missing"))?;
    if !["text","input_text","output_text"].contains(&kind) || blocks.iter().any(|block|
        !block.as_object().is_some_and(|map|map.len()==2 && map.contains_key("type") && map.contains_key("text"))
            || block["type"] != kind || !block["text"].is_string()) {
        return Err(CoreError::new("native_reorder_unsupported","reorder cannot cross an opaque or mixed native block boundary",501));
    }
    let indices:BTreeSet<usize> = context.units().iter().filter(|unit|editable.contains(&unit.unit_id)).filter_map(|unit| {
        let other=unit.anchor.as_array()?;
        (other.len()==path.len() && &other[..other.len()-2]==parent).then(||other[other.len()-2].as_u64().map(|n|n as usize)).flatten()
    }).collect();
    let mut start=index;
    while start>0 && indices.contains(&(start-1)) {start-=1;}
    Ok(format!("{}:{start}",serde_json::to_string(parent).map_err(|_|CoreError::invalid("native parent encoding failed"))?))
}
pub(crate) fn project(context:&NativeContext,editable:&[String],ops:&Value,pvi:&Value) -> Result<Value> {
    let units:Vec<_>=context.units().iter().filter(|unit|editable.contains(&unit.unit_id)).collect();
    let source=json!({"units":units.iter().map(|unit|json!({"unit_id":unit.unit_id,"text":unit.text,"anchor":unit.anchor})).collect::<Vec<_>>()});
    let assignments=json!([{"editor":"editor-1","unit_ids":editable}]);
    let response=json!({"editor-1":{"ops":ops,"pvi":pvi}});
    let material=editing::materialize(&source,&assignments,&response,true).map_err(CoreError::invalid)?;
    let operations=ops.as_array().ok_or_else(||CoreError::invalid("ops must be an array"))?;
    let mut active:BTreeSet<String>=editable.iter().cloned().collect();
    let mut order:BTreeMap<String,i128>=units.iter().enumerate().map(|(i,unit)|(unit.unit_id.clone(),i as i128)).collect();
    let mut appended=Vec::<String>::new();
    for op in operations {
        let kind=text(op,"op")?;
        let life=op["lifecycle"].as_str().unwrap_or("P");
        if kind=="ADD" && life=="P" && op.get("anchor").is_none() {appended.push(text(op,"content")?.into());}
        if kind=="ADD" {continue;}
        let target=text(op,"target")?;
        if kind=="DELETE" || (["REPLACE","COMPRESS"].contains(&kind) && life!="P") {active.remove(target);}
        if ["REPLACE","COMPRESS"].contains(&kind) && life=="P" {active.insert(target.into());}
        if kind=="REORDER" {
            let number=op["order"].as_i64().map(i128::from).or_else(||op["order"].as_u64().map(i128::from))
                .ok_or_else(||CoreError::invalid("reorder order must be integer"))?;
            order.insert(target.into(),number);
        }
    }
    appended.extend(pvi["P"].as_array().into_iter().flatten().map(|item|item.as_str().expect("materializer validated P").to_owned()));
    let mut pieces=BTreeMap::<String,String>::new();
    for unit in &units {
        let scoped:Vec<Value>=operations.iter().filter(|op|op["target"]==unit.unit_id ||
            (op["op"]=="ADD" && op["anchor"]["target"]==unit.unit_id)).cloned().collect();
        let one=json!({"units":[{"unit_id":unit.unit_id,"text":unit.text,"anchor":unit.anchor}]});
        let rendered=editing::materialize(&one,&json!([{"editor":"editor-1","unit_ids":[unit.unit_id]}]),
            &json!({"editor-1":{"ops":scoped,"pvi":{}}}),true).map_err(CoreError::invalid)?;
        pieces.insert(unit.unit_id.clone(),rendered["body"].as_str().expect("materializer body").into());
    }
    let slots:Vec<_>=units.iter().filter(|unit|active.contains(&unit.unit_id)).copied().collect();
    let mut sorted=slots.clone();
    sorted.sort_by_key(|unit|(order[&unit.unit_id],units.iter().position(|source|source.unit_id==unit.unit_id).unwrap()));
    let mut projected:BTreeMap<String,String>=units.iter().map(|unit|(unit.unit_id.clone(),String::new())).collect();
    let mut occupied:BTreeSet<String>=slots.iter().map(|unit|unit.unit_id.clone()).collect();
    let mut movement=Vec::new();
    for (slot,source) in slots.iter().zip(&sorted) {
        if slot.unit_id!=source.unit_id && parent_group(context,&slot.unit_id,editable)? != parent_group(context,&source.unit_id,editable)? {
            return Err(CoreError::new("native_reorder_unsupported","reorder cannot cross native message, role, tool or read-only boundaries",501));
        }
        projected.insert(slot.unit_id.clone(),pieces[&source.unit_id].clone());
        movement.push(json!({"source_unit_id":source.unit_id,"native_unit_id":slot.unit_id,"native_anchor":slot.anchor}));
    }
    let mut generated_append=Value::Null;
    if !appended.is_empty() {
        let destination=slots.last().copied().or_else(||units.last().copied())
            .ok_or_else(||CoreError::new("native_projection_unsupported","generated P needs an authorized native text leaf",501))?;
        let value=projected.get_mut(&destination.unit_id).expect("native destination");
        if !slots.is_empty() {value.push('\n');}
        value.push_str(&appended.join("\n"));
        occupied.insert(destination.unit_id.clone());
        generated_append=json!({"native_unit_id":destination.unit_id,"native_anchor":destination.anchor,
            "generated_text_sha256":crate::storage::sha256(appended.join("\n").as_bytes())});
    }
    let reconstructed=units.iter().filter(|unit|occupied.contains(&unit.unit_id))
        .map(|unit|projected[&unit.unit_id].clone()).collect::<Vec<_>>().join("\n");
    if material["body"].as_str()!=Some(reconstructed.as_str()) {
        return Err(CoreError::conflict("native leaf projection does not reproduce the authoritative editing body"));
    }
    Ok(json!({"texts":projected,"movement":movement,"generated_append":generated_append,"material":material}))
}
