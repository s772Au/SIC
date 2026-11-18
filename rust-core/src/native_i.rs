//! Independent host-program projection of the current I into its registered readonly native leaf.
use crate::{core::{Core,OperationContext},error::{CoreError,Result},storage,transparent::NativeContext};
use serde_json::{Value,json};
use sqlx::{Sqlite,Transaction,Row};
pub(crate) async fn latest(tx:&mut Transaction<'_,Sqlite>,id:&str,version:u64)->Result<Option<Value>> {
 let id:Option<String>=sqlx::query_scalar("SELECT id FROM objects WHERE kind='native_i_projection' AND json_extract(data,'$.source_id')=? AND json_extract(data,'$.candidate_version')=? ORDER BY CAST(json_extract(data,'$.revision') AS INTEGER) DESC LIMIT 1")
  .bind(id).bind(version as i64).fetch_optional(&mut **tx).await?;
 match id {Some(id)=>Ok(Some(storage::get_object(tx,&id,Some("native_i_projection")).await?)),None=>Ok(None)}
}
pub(crate) async fn refresh(core:&Core,tx:&mut Transaction<'_,Sqlite>,auth:&OperationContext,source:&Value,current:&NativeContext,p:&Value,publication:&Value)->Result<(Value,bool)> {
 let source_id=source["source_id"].as_str().or_else(||p["source_id"].as_str()).ok_or_else(||CoreError::invalid("source_id missing"))?;
 let previous=latest(tx,source_id,current.version()).await?;
 if let Some(previous)=&previous {core.authorize_native_projection(tx,auth,previous).await?;}
 let mut plan=core.plan_native_i_projection(tx,auth,source,current,p,publication,previous.as_ref()).await?;
 let fingerprint=storage::digest(&json!([auth.actor,auth.chain_id,p,plan["view_digest"],publication["publication_id"]]))?;
 let identity=match p.get("idempotency_key") {Some(value)=>value.as_str().filter(|value|!value.is_empty()).ok_or_else(||CoreError::invalid("idempotency_key must be nonempty text"))?,None=>&fingerprint};
 let key=format!("native-I:{}",storage::digest(&json!([auth.actor,auth.chain_id,source_id,identity]))?);
 if let Some(prior)=sqlx::query("SELECT fingerprint,result FROM receipts WHERE key=?").bind(&key).fetch_optional(&mut **tx).await? {
  if prior.try_get::<String,_>("fingerprint")?!=fingerprint {return Err(CoreError::conflict("I projection key or current view changed"));}
  let saved:Value=serde_json::from_str(&prior.try_get::<String,_>("result")?)?;
  let projection=storage::get_object(tx,saved["projection_id"].as_str().ok_or_else(||CoreError::invalid("projection receipt missing"))?,Some("native_i_projection")).await?;
  if previous.as_ref().is_some_and(|previous|previous["projection_id"]!=projection["projection_id"]) {return Err(CoreError::conflict("I projection receipt was superseded"));}
  core.authorize_native_projection(tx,auth,&projection).await?;
  return Ok((projection,true));
 }
 let revision=previous.as_ref().map(|previous|previous["revision"].as_u64().unwrap_or(0)).unwrap_or(0).checked_add(1).ok_or_else(||CoreError::conflict("I projection revision exhausted"))?;
 let id=storage::uid("native_i_projection");
 plan["projection_id"]=json!(id);plan["revision"]=json!(revision);plan["actor"]=json!(auth.actor);plan["chain_id"]=json!(auth.chain_id);plan["fingerprint"]=json!(fingerprint);
 storage::put_object(tx,"native_i_projection",&id,&plan).await?;
 sqlx::query("INSERT INTO receipts(key,fingerprint,result) VALUES(?,?,?)").bind(key).bind(fingerprint).bind(storage::json_text(&json!({"projection_id":id}))?).execute(&mut **tx).await?;
 Ok((plan,false))
}
