import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {setTimeout as wait} from 'node:timers/promises';
import {createCoreClient} from '../transparent/core-client.mjs';

test('a stalled actual Core response body has a deadline without retrying a mutation or grant',async()=>{
  let calls=0;const server=createServer((_request,response)=>{calls++;response.writeHead(200,{'content-type':'application/json'});response.write('{');});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  try{
    const core=createCoreClient({baseURL:`http://127.0.0.1:${server.address().port}`,credential:()=> 'owned-fixture',context:()=>({actor:'host'}),requestTimeoutMs:70});
    for(const [operation,run] of [['chain.create',()=>core.operate('chain.create',{},AbortSignal.timeout(1500))],['harness.turn',()=>core.issueTurn({},AbortSignal.timeout(1500))]]){
      const started=Date.now();
      await assert.rejects(run(),error=>error.name==='TimeoutError'&&error.message.includes(operation));
      assert.ok(Date.now()-started<1000,'Core body deadline must precede the caller fallback');
    }
    assert.equal(calls,2,'uncertain mutations must not be automatically retried');
  }finally{server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}
});

test('the original host cancellation reason survives the Core RPC deadline composition',async()=>{
  const controller=new AbortController(),reason=new Error('host cancelled this control operation');controller.abort(reason);
  const core=createCoreClient({baseURL:'http://127.0.0.1:1',credential:()=> 'owned-fixture',context:()=>({actor:'host'}),requestTimeoutMs:100});
  await assert.rejects(core.get({source_id:'s'},controller.signal),error=>error===reason);
});

test('a longer hierarchy deadline applies only to the Core schedule operation',async()=>{
  const calls=[];
  const core=createCoreClient({baseURL:'http://127.0.0.1:1',credential:()=> 'owned-fixture',context:()=>({actor:'host'}),
    requestTimeoutMs:10,operationTimeoutMs:{'runtime.schedule':500},fetchImpl:async(_url,options)=>{
      calls.push(JSON.parse(options.body).operation);
      await wait(40,undefined,{signal:options.signal});
      return Response.json({result:{scheduled:true}});
    }});
  await assert.rejects(core.operate('chain.get',{}),/timed out after 10 ms/);
  assert.deepEqual(await core.operate('runtime.schedule',{}),{scheduled:true});
  assert.deepEqual(calls,['chain.get','runtime.schedule']);
});
