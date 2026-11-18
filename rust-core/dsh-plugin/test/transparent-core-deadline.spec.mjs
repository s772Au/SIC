import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createCoreClient} from '../transparent/core-client.mjs';

test('a stalled actual Core response body has a deadline without retrying a mutation or grant',async()=>{
  let calls=0;const server=createServer((_request,response)=>{calls++;response.writeHead(200,{'content-type':'application/json'});response.write('{');});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  try{
    const core=createCoreClient({baseURL:`http://127.0.0.1:${server.address().port}`,credential:()=> 'owned-fixture',context:()=>({actor:'host'}),requestTimeoutMs:70});
    for(const run of [()=>core.operate('chain.create',{},AbortSignal.timeout(1500)),()=>core.issueTurn({},AbortSignal.timeout(1500))]){
      const started=Date.now();await assert.rejects(run(),error=>error.name==='TimeoutError');assert.ok(Date.now()-started<1000,'Core body deadline must precede the caller fallback');
    }
    assert.equal(calls,2,'uncertain mutations must not be automatically retried');
  }finally{server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}
});

test('the original host cancellation reason survives the Core RPC deadline composition',async()=>{
  const controller=new AbortController(),reason=new Error('host cancelled this control operation');controller.abort(reason);
  const core=createCoreClient({baseURL:'http://127.0.0.1:1',credential:()=> 'owned-fixture',context:()=>({actor:'host'}),requestTimeoutMs:100});
  await assert.rejects(core.get({source_id:'s'},controller.signal),error=>error===reason);
});
