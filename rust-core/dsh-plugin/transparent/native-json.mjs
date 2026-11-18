// Locate JSON string VALUES without reserializing opaque native JSON carriers.
// Native metadata, numeric lexemes, signatures and field order remain byte exact.
export function applyRawTextEdits(raw,units,edits){
  JSON.parse(raw); // Full grammar validation; scanner only records string spans.
  let i=0;const spans=new Map(),duplicates=new Set();
  const white=()=>{while(/\s/u.test(raw[i]??'')&&i<raw.length)i++;};
  function string(){
    const start=i++;
    while(i<raw.length){if(raw[i]==='\\'){i+=2;continue;}if(raw[i++]==='"')return{start,end:i,text:JSON.parse(raw.slice(start,i))};}
    throw Error('unterminated native JSON string');
  }
  function value(path){
    white();
    if(raw[i]==='"'){
      const span=string(),key=JSON.stringify(path);
      if(spans.has(key))duplicates.add(key);spans.set(key,span);return;
    }
    if(raw[i]==='{'){
      i++;white();if(raw[i]==='}'){i++;return;}
      while(i<raw.length){white();const key=string().text;white();i++;value([...path,key]);white();if(raw[i++]==='}')return;}
    }else if(raw[i]==='['){
      i++;white();if(raw[i]===']'){i++;return;}
      let index=0;while(i<raw.length){value([...path,index++]);white();if(raw[i++]===']')return;}
    }else{while(i<raw.length&&!/[\s,}\]]/u.test(raw[i]))i++;return;}
    throw Error('invalid native JSON structure');
  }
  value([]);white();if(i!==raw.length)throw Error('trailing native JSON material');
  const byId=new Map(units.map(u=>[u.unit_id,u])),changes=[];
  for(const edit of edits){
    const unit=byId.get(edit.unit_id),key=JSON.stringify(unit?.anchor),span=spans.get(key);
    if(!unit||!span||duplicates.has(key)||span.text!==unit.text||span.text!==edit.expected_text)throw Error('ambiguous native raw text anchor');
    changes.push({...span,replacement:JSON.stringify(edit.replacement)});
  }
  changes.sort((a,b)=>b.start-a.start);
  let output=raw,last=raw.length;
  for(const c of changes){if(c.end>last)throw Error('overlapping native raw edits');output=output.slice(0,c.start)+c.replacement+output.slice(c.end);last=c.start;}
  return output;
}
