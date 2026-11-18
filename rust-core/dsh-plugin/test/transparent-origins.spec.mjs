import{test}from'node:test';import assert from'node:assert/strict';
import{messagesOrigins,reconcileFrozenOrigins}from'../transparent/origins.mjs';
import{editorDelivery}from'../transparent/editor-delivery.mjs';

test('native same-role merge maps immutable IDs, replays old CB, and grants only new material',()=>{
  const history=[{id:'old',role:'user',source:{kind:'user'},content:[{type:'text',text:'same words'}]},
    {id:'runtime',role:'user',source:{kind:'runtime-context'},content:[{type:'text',text:'trusted activation'}]},
    {id:'new',role:'user',source:{kind:'user'},content:[{type:'text',text:'same words'}]}];
  const request={messages:[{role:'user',content:[{type:'text',text:'same words'},{type:'text',text:'trusted activation'},{type:'text',text:'same words'}]}]};
  const origins=messagesOrigins(history,request);
  const mapped=reconcileFrozenOrigins(request,origins,[{cb_id:'cb-old',leaves:[{message_id:'old',block_index:0,original_text:'same words',adopted_text:'fixed CB'}]}]);
  assert.deepEqual(mapped.editable_anchors,[['messages',0,'content',2,'text']]);
  assert.equal(mapped.request.messages[0].content[0].text,'fixed CB');
  assert.deepEqual(mapped.protected_cb_bindings,[{cb_id:'cb-old',native_anchors:[['messages',0,'content',0,'text']]}]);
  assert.throws(()=>reconcileFrozenOrigins(request,origins,[{cb_id:'cb',leaves:[{message_id:'missing',block_index:0,original_text:'same words',adopted_text:'x'}]}]),/identity missing/);
});

test('Editor delivery refuses old CB target and unmapped primitive even when source text equals new material',()=>{
  const source={units:[{unit_id:'u1',text:'same',anchor:['messages',0,'content']},{unit_id:'u2',text:'same',anchor:['messages',1,'content']}]};
  const allowed=[['messages',1,'content']];
  assert.throws(()=>editorDelivery(source,{ops:[{op:'REPLACE',target:'u1',content:'bad'}],pvi:{}},allowed),/outside assigned/);
  assert.throws(()=>editorDelivery(source,{ops:[{op:'DELETE',target:'u2'}],pvi:{}},allowed),/not mapped/);
  assert.deepEqual(editorDelivery(source,{ops:[{op:'COMPRESS',target:'u2',content:'short'}],pvi:{I:{status:'s'}}},allowed).edits,[{unit_id:'u2',expected_text:'same',replacement:'short'}]);
});
