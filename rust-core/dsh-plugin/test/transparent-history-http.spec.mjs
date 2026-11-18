import {evidencePath} from './test-paths.mjs';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {isolated} from './isolated-core-fixture.mjs';
import {createCoreClient} from '../transparent/core-client.mjs';
import {createNativeProjectionHandler,createProjectionHistoryReader} from '../transparent/projection.mjs';

test('projection API pages all 26 real Core candidate versions and 25 I receipts with independent cursors and permissions',{timeout:30000},async()=>{
  const host=await isolated({allowRevoke:true,evidenceRoot:process.env.SIC_HISTORY_EVIDENCE_ROOT??evidencePath('production-readiness-20261118/core/history-ui')});
  const {core,root,baseURL,chain_id,credential}=host;
  try{
    const cut=await core.operate('cut.get',{}),section='sic:instant-views\n'+cut.instant_view_text;
    const source=await core.begin({format:'dsh-messages',request:{opaque:{vendor:'unchanged'},messages:[
      {id:'new',role:'user',content:[{type:'text',text:'v0'}]},
      {id:'runtime-I',role:'user',content:[{type:'text',text:section}]}]},
      editable_anchors:[['messages',0,'content',0,'text']],
      adopted_i:{cut_id:cut.cut_id,view_digest:cut.instant_view_digest,native_anchor:['messages',1,'content',0,'text'],section_text:section},
      host_message_origins:[{message_id:'runtime-I',block_index:0,native_anchor:['messages',1,'content',0,'text']}]});
    for(let version=0;version<25;version++)await core.commit({source_id:source.source_id,expected_source_digest:source.source_digest,expected_version:version,
      edits:[{unit_id:'u1',expected_text:'v'+version,replacement:'v'+(version+1)}]});
    const publication=await core.publish({source_id:source.source_id,expected_source_digest:source.source_digest,expected_version:25,expected_cut:0,idempotency_key:'history-pages'});
    const current=await core.operate('cut.get',{});
    for(let n=1;n<=25;n++)await core.refreshI({source_id:source.source_id,expected_version:25,cut_id:current.cut_id,idempotency_key:'history-I-'+n});
    const record={id:'projection',source_id:source.source_id},traceRows=Array.from({length:27},(_,n)=>({stage:'editor-chunk',trace_sequence:n+1}));
    const reader=createProjectionHistoryReader({clientFor:()=>core,traces:async(_id,_source,offset,limit)=>({rows:traceRows.slice(offset,offset+limit),next_offset:offset+limit<traceRows.length?offset+limit:null})});
    const handler=createNativeProjectionHandler({authorize:async(_request,id)=>id==='session',list:async()=>[],get:async()=>{await core.get({source_id:source.source_id});return record;},history:reader});
    const call=body=>handler(new Request('http://host/api/sic-core/native-projections',{method:'POST',body:JSON.stringify({sessionId:'session',action:'history',projectionId:'projection',...body})}));
    const first=await (await call({})).json();
    assert.equal(first.value.core.versions.length,20);assert.equal(first.value.core.i_projections.length,20);
    assert.equal(first.value.core.next_after_version,19);assert.equal(first.value.core.next_after_i_revision,20);assert.equal(first.value.editor.next_offset,20);
    const second=await (await call({limit:7,offset:20,after_version:19,after_i_revision:20})).json();
    assert.deepEqual(second.value.core.versions.map(v=>v.version),[20,21,22,23,24,25]);
    assert.deepEqual(second.value.core.i_projections.map(v=>v.revision),[21,22,23,24,25]);
    assert.deepEqual(second.value.editor.rows.map(v=>v.trace_sequence),[21,22,23,24,25,26,27]);
    assert.equal(second.value.core.has_more,false);assert.equal(second.value.core.has_more_i,false);assert.equal(second.value.editor.next_offset,null);
    assert.deepEqual(second.value.core.publication,publication.publication);
    assert.equal(second.value.core.source.request.messages[0].content[0].text,'v0');
    assert.deepEqual(second.value.core.versions.at(-1).request.opaque,{vendor:'unchanged'});
    assert.equal((await call({sessionId:'another'})).status,403);
    const foreign=createCoreClient({baseURL,credential,context:()=>({actor:'native-http-actor',chain_id:null,allow_write:false})});
    await assert.rejects(foreign.history({source_id:source.source_id,after_version:19,after_i_revision:20}),/forbidden/);
    const revoker=createCoreClient({baseURL,credential,context:()=>({actor:'native-http-actor',chain_id,allow_write:true,allow_revoke:true})});
    await revoker.operate('security.revoke',{source_id:source.binding.core_source_id});
    await assert.rejects(call({limit:7,after_version:19,after_i_revision:20}),/forbidden/);
    await writeFile(join(root,'history-pages.json'),JSON.stringify({source_id:source.source_id,publication_id:publication.publication.publication_id,
      versions:[...first.value.core.versions,...second.value.core.versions].map(v=>v.version),i_revisions:[...first.value.core.i_projections,...second.value.core.i_projections].map(v=>v.revision),
      unauthorized_session:403,foreign_chain:'forbidden',revoked_source:'forbidden'},null,2));
  }finally{await host.close();}
});
