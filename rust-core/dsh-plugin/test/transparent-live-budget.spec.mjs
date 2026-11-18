import {test} from 'node:test';import assert from 'node:assert/strict';
import {reservePaidRequest,paidRowUpperCost,autoDL} from './autodl-live-support.mjs';
test('paid opt-in request admission counts failed/unknown calls and reserves full output even when usage is missing or zero',()=>{
 const request={model:'authorized',max_tokens:4096},options={model:'authorized',maxRequests:30};
 const reserve=reservePaidRequest({requests:[]},request,1000,options);assert.ok(reserve>0.032);
 assert.throws(()=>reservePaidRequest({requests:Array.from({length:30},()=>({reserved_upper_cny:reserve,status:500}))},request,1000,options),/budget exhausted/);
 assert.throws(()=>reservePaidRequest({requests:[{reserved_upper_cny:4.99,usage:{inputTokens:0,outputTokens:0}}]},request,1000,options),/budget exhausted/);
 assert.throws(()=>reservePaidRequest({requests:[]},{...request,max_tokens:4097},1000,options),/output budget mismatch/);
 assert.throws(()=>reservePaidRequest({requests:[]},{...request,model:'other-provider-model'},1000,options),/model\/output budget mismatch/);
});
test('authorized extension allows sixty total attempts while retaining the currency ceiling and unknown reservations',async()=>{
 const row={reserved_upper_cny:0.01},request={model:'authorized',max_tokens:128},options={model:'authorized',maxRequests:60};
 assert.ok(reservePaidRequest({requests:Array.from({length:59},()=>row)},request,100,options)>0);
 assert.throws(()=>reservePaidRequest({requests:Array.from({length:60},()=>row)},request,100,options),/budget exhausted/);
 assert.equal(paidRowUpperCost({...row,usage:{inputTokens:100,outputTokens:10,cacheReadTokens:20,cacheWriteTokens:5}}),0.00033);
 for(const usage of [undefined,{inputTokens:0,outputTokens:0},{inputTokens:100,outputTokens:NaN},{inputTokens:-1,outputTokens:10}])assert.equal(paidRowUpperCost({...row,usage}),0.01);
 assert.throws(()=>reservePaidRequest({requests:Array.from({length:50},()=>({reserved_upper_cny:0.1}))},request,100,options),/budget exhausted/);
 await assert.rejects(autoDL({maxRequests:61}),/between 1 and 60/);
});
