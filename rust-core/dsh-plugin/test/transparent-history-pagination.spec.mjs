import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {runInNewContext} from 'node:vm';
import {createNativeProjectionHandler,createProjectionHistoryReader} from '../transparent/projection.mjs';

const request=body=>new Request('http://host/api/sic-core/native-projections',{method:'POST',body:JSON.stringify(body)});

test('projection history forwards bounded Core cursors independently of trace offsets; hierarchy stays host-owned',async()=>{
  const calls=[],traces=[];
  const history=createProjectionHistoryReader({clientFor:id=>({history:async p=>{calls.push([id,p]);return p;}}),
    traces:async(...args)=>{traces.push(args);return{rows:[],next_offset:null};}});
  const record={id:'p',source_id:'native-source'};
  const handler=createNativeProjectionHandler({authorize:async(_r,id)=>id==='session',list:async()=>[],get:async()=>record,history});
  const body={sessionId:'session',action:'history',projectionId:'p',offset:307,limit:7,after_version:23,after_i_revision:41};
  const response=await handler(request(body));assert.equal(response.status,200);
  assert.deepEqual(calls,[['session',{source_id:'native-source',limit:7,after_version:23,after_i_revision:41}]]);
  assert.deepEqual(traces,[['session','native-source',307,7]]);
  for(const [key,value] of [['after_version',-1],['after_i_revision',1.2],['after_version',null],['after_i_revision',Number.MAX_SAFE_INTEGER+1]]){
    assert.equal((await handler(request({...body,[key]:value}))).status,400);
  }
  assert.equal((await handler(request({...body,action:'get'}))).status,400);
  assert.equal((await handler(request({...body,sessionId:'other'}))).status,403);
  const hierarchy={job_id:'job',source_id:'core-source'};
  assert.equal((await history('session',{source_id:'core-source',source_kind:'host-hierarchy',hierarchy},9,3,{after_version:2})).core,hierarchy);
  assert.equal(calls.length,1,'a hierarchy source must never be sent to native.history');
});

test('NativeTrace independently pages and merges versions, I and legacy trace rows without mixing cursors',async()=>{
  const states=[],refs=[],calls=[];let stateIndex=0,refIndex=0,view;
  const React={Fragment:'fragment',createElement:(type,props,...children)=>({type,props:props??{},children}),
    useState:initial=>{const i=stateIndex++;if(!(i in states))states[i]=initial;return[states[i],value=>{states[i]=typeof value==='function'?value(states[i]):value;}];},
    useRef:initial=>refs[refIndex++]??(refs[refIndex-1]={current:initial}),useEffect:()=>{}};
  const versions=Array.from({length:205},(_,version)=>({version})),iRows=Array.from({length:202},(_,i)=>({revision:i+1}));
  // Legacy rows intentionally have no id/trace_sequence: all rows must survive paging.
  const traceRows=Array.from({length:203},(_,n)=>({stage:'editor-chunk',text:String(n)}));
  const fetch=async(_url,init)=>{
    const b=JSON.parse(init.body);calls.push(b);
    const page=(rows,key,after)=>rows.filter(row=>row[key]>after).slice(0,b.limit);
    let value;
    if(b.action==='list')value={rows:[{id:'p',format:'dsh-messages',request_id:'request',stage:'admitted'}],next_offset:null};
    else if(b.action==='get')value={id:'p',source_id:'src'};
    else{
      const v=page(versions,'version',b.after_version??-1),i=page(iRows,'revision',b.after_i_revision??0),rows=traceRows.slice(b.offset,b.offset+b.limit);
      const has_more=v.at(-1)?.version<204,has_more_i=i.at(-1)?.revision<202;
      value={core:{versions:v,i_projections:i,has_more,has_more_i,next_after_version:has_more?v.at(-1).version:null,next_after_i_revision:has_more_i?i.at(-1).revision:null},
        editor:{rows,next_offset:b.offset+rows.length<traceRows.length?b.offset+rows.length:null}};
    }
    return Response.json({ok:true,value});
  };
  const window={__ModuleLoader__:{load:({factory})=>{
    const plugin=factory(()=>React);
    plugin.apply({effect:fn=>fn(),slots:{inject:(_name,fn)=>fn(),register:(_meta,render)=>{view=render({sessionId:'session'}).type;}}});
  }}};
  runInNewContext(await readFile(new URL('../transparent/native-client.js',import.meta.url),'utf8'),{window,fetch});
  const render=()=>{stateIndex=0;refIndex=0;return view({sessionId:'session'});};
  const find=(node,label)=>node&&typeof node==='object'?(node.type==='button'&&node.children.includes(label)?node:node.children.flat(Infinity).map(child=>find(child,label)).find(Boolean)):null;
  const click=async label=>{const button=find(render(),label);assert.ok(button,label);button.props.onClick();await new Promise(setImmediate);};
  const history=()=>JSON.parse(JSON.stringify(states[3]));
  await click('读取投影');await click('dsh-messages · request · admitted');await click('读取来源版本与发布历史');
  assert.equal(history().core.versions.length,100);assert.equal(history().core.i_projections.length,100);
  await click('读取下一页候选版本');
  assert.equal(calls.at(-1).after_version,99);assert.equal(calls.at(-1).after_i_revision,100);assert.equal(calls.at(-1).offset,0);
  assert.equal(history().core.versions.length,200);assert.equal(history().core.i_projections.length,100);assert.equal(history().editor.rows.length,100);
  await click('读取下一页 I 刷新');
  assert.equal(calls.at(-1).after_version,199);assert.equal(calls.at(-1).after_i_revision,100);
  assert.equal(history().core.i_projections.length,200);assert.equal(history().core.versions.length,200);
  await click('读取下一页流程');
  assert.equal(calls.at(-1).offset,100);assert.equal(calls.at(-1).after_version,199);assert.equal(calls.at(-1).after_i_revision,200);
  await click('读取下一页候选版本');await click('读取下一页 I 刷新');await click('读取下一页流程');
  assert.deepEqual(history().core.versions,versions);assert.deepEqual(history().core.i_projections,iRows);assert.deepEqual(history().editor.rows,traceRows);
  assert.equal(find(render(),'读取下一页候选版本'),undefined);assert.equal(find(render(),'读取下一页 I 刷新'),undefined);assert.equal(find(render(),'读取下一页流程'),undefined);
});
