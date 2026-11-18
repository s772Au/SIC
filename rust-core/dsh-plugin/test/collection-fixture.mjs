export async function collectedFixture(core,chain_id,scope_id='scope'){
 const base={scope_id,chain_id,human_intent:'none',human_message_id:null,goal_round:null,max_goal_rounds:null,wait_timeout_ms:null};let n=0;
 const turn=(name,role,object_id)=>core.issueTurn({...base,turn_id:'fixture-'+(++n),role,allowed_calls:[name],object_id});
 const g=await turn('job.register','worker',null),r=await core.operate('harness.observe',{scope_id,grant_id:g.grant_id,event_id:'register',observation:{kind:'job.register',external_id:'fixture-native-id',job_kind:'fixture',label:'collection source',started_at_ms:Date.now(),output_mode:'stream',executor:{kind:'fixture',id:'fixture-native-id'}}}),id=r.object_id;
 async function observe(kind,extra){const state=await core.operate('harness.get',{scope_id,object_id:id}),g=await turn(kind,'executor',id);return core.operate('harness.observe',{scope_id,object_id:id,expected_revision:state.revision,grant_id:g.grant_id,event_id:'event-'+n,observation:{kind,...extra}});}
 await observe('job.output',{text:'真实收集正文'});await observe('job.settle',{status:'completed',finished_at_ms:Date.now(),result:'done'});
 const grant=await turn('job_output','worker',id),result=await core.operate('harness.call',{scope_id,name:'job_output',arguments:{job_id:'fixture-native-id'},grant_id:grant.grant_id,event_id:'collection'});
 return{id,result,proof:{lifecycle_object_id:id,collection_receipt_id:result.receipt_id,collection_digest:result.collection.text_digest,chain_id}};
}
