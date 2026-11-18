import assert from 'node:assert/strict';

const cases = [
  {key:'relay', targets:['B','D'], codes:['C8J5','K7M4','L6X2','P3V9']},
  {key:'meter', targets:['C','A'], codes:['F6Q3','T2W8','H9K5','R7N4']},
  {key:'panel', targets:['D','A'], codes:['Y6B8','G3T9','J5H2','W4F7']},
  {key:'rack', targets:['B','C'], codes:['V2J8','N8C6','M5Y3','K9T4']},
];
const panels=['A','B','C','D'];
export const imageCases=cases.map((row,index)=>({
  ...row,index,
  task:'Read the four-character code in panel '+row.targets[0]+' and then panel '+row.targets[1]+
    '. Use the evidence supplied. Return only a JSON array of two strings in that order. Do not include Markdown. If a code is unreadable, use "?".',
  answer:row.targets.map(label=>row.codes[panels.indexOf(label)]),
}));
export async function renderCase(sharp,row){
  const width=640,height=448;
  const entries=panels.map((label,index)=>{
    const x=28+(index%2)*310,y=28+Math.floor(index/2)*210;
    return{label,x,y,width:274,height:178,code:row.codes[index],key:{x:x+34,y:y+56,width:206,height:98}};
  });
  const svg='<svg xmlns="http://www.w3.org/2000/svg" width="'+width+'" height="'+height+'"><rect width="100%" height="100%" fill="#e9edf4"/>'+
    entries.map(({label,x,y,width,height,code})=>'<g transform="translate('+x+' '+y+')"><rect width="'+width+'" height="'+height+'" rx="9" fill="white" stroke="#2a4660" stroke-width="4"/><text x="16" y="38" font-size="23" font-family="Arial" font-weight="bold" fill="#23425c">PANEL '+label+'</text><text x="137" y="130" text-anchor="middle" font-size="59" font-family="Courier New" font-weight="bold" fill="#191f2c">'+code+'</text></g>').join('')+'</svg>';
  const png=await sharp(Buffer.from(svg)).png().toBuffer();
  return {png,width,height,targets:row.targets.map(label=>({label,region:entries.find(e=>e.label===label).key}))};
}
export function score(text,expected){
  let answer;try{answer=JSON.parse(text);}catch{answer=null;}
  const valid=Array.isArray(answer)&&answer.length===expected.length&&answer.every(value=>typeof value==='string');
  return{answer,format_ok:valid,correct_items:valid?expected.filter((value,index)=>value===answer[index]).length:0,
    expected_items:expected.length,exact:valid&&expected.every((value,index)=>value===answer[index])};
}
export function coverage(targets,crops){
  return targets.map(({label,region:t})=>({
    label,target:t,
    max_coverage:Math.max(0,...crops.map(({region:c})=>Math.max(0,Math.min(t.x+t.width,c.x+c.width)-Math.max(t.x,c.x))*
      Math.max(0,Math.min(t.y+t.height,c.y+c.height)-Math.max(t.y,c.y))/(t.width*t.height))),
  }));
}
export function assertNoAnswerInText(parts,codes){
  const text=parts.join('\n').toUpperCase();
  for(const code of codes)assert.ok(!text.includes(code),'visual-only teacher/original prompt leaked a fixture code');
}
