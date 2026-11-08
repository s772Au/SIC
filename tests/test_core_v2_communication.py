import concurrent.futures,copy,json,unittest,os,threading,urllib.request,urllib.error
from sic_core.server import make_server
from unittest.mock import patch
from sic_core import Core,CoreError
from sic_core import adapters,communication

class CommunicationTests(unittest.TestCase):
 def setUp(self):
  self.core=Core(config={'revocation_actors':['host']});self.ctx={'actor':'host','allow_write':True,'allow_model':False};self.ctx['chain_id']=self.op('chain.create')['chain_id'];self.publish(self.raw('retained original'));self.cp=self.op('chain.checkpoint')['checkpoint_id'];self.sock=self.op('socket.create',{'checkpoint_id':self.cp,'spawn_acl':['host'],'message_acl':['host']})['socket_id']
 def tearDown(self):self.core.close()
 def op(self,n,p=None):return self.core.operate(n,self.ctx,p or {})['result']
 def raw(self,t):return self.op('source.ingest',{'request':{'messages':[{'role':'user','content':t}]}})['source_id']
 def publish(self,sid):
  j=self.op('edit.program',{'source_id':sid})['job_id'];return self.op('cut.publish',{'job_id':j,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':j})
 def send(self,mid='message1',life='P',text='new withheld communication'):
  return self.op('socket.message',{'socket_id':self.sock,'message_id':mid,'lifecycle':life,'text':text})
 def request(self):
  with self.core.store.transaction():return adapters.encode(self.core._working_view(self.core.store.state(self.ctx['chain_id']),'worker'))
 def test_receipt_repeat_notify_and_no_same_cut_prefix_change(self):
  before=self.request();self.assertNotIn('new withheld communication',json.dumps(before));sid=self.raw('next working block');before=self.request()
  with patch.object(self.core.runtime,'notify') as notify:
   a=self.send();b=self.send();self.assertEqual(a,b);self.assertEqual(notify.call_count,1);event=notify.call_args.args[2];self.assertEqual(event['message_id'],'message1');self.assertEqual(event['received_event_id'],a['received_event_id'])
  self.assertEqual(before,self.request());state=self.op('chain.get');self.assertEqual(len([e for e in state['events'] if e.get('type')=='communication_received']),1)
  old=self.core.store.get(state['current_cut'],'cut');out=self.publish(sid);self.assertEqual(old,self.core.store.get(old['cut_id'],'cut'));cb=self.core.store.get(out['cb_id'],'cb');record=next(o for o in cb['ops'] if o.get('communication'));self.assertEqual(record['communication']['received_at'],a['received_at']);self.assertIn('new withheld communication',json.dumps(self.request()))
  page=self.op('memory.page',{'kind':'communication','object_id':out['cut_id'],'limit':8192});self.assertEqual(json.loads(page['value'])['communications'][0]['receive_receipt_id'],a['receive_receipt_id']);self.assertNotIn('new withheld communication',page['value'])
 def test_volatile_editor_readonly_source_and_paging(self):
  a=self.send(life='V');out=self.publish(self.raw('consume queue'));src=self.core.store.get(out['volatile_source_id'],'source');self.assertEqual(src['communications'][0]['receive_receipt_id'],a['receive_receipt_id']);job=self.op('edit.prepare',{'source_id':out['volatile_source_id']})['job_id']
  with self.core.store.transaction():
   state=self.core.store.state(self.ctx['chain_id']);j=state['jobs'][job];req=self.core._editor_request(j,j['assignments'][0]['editor'])
  self.assertIn(a['receive_receipt_id'],json.dumps(req));self.assertTrue(all(set(u)=={'unit_id','text'} for u in json.loads(req['messages'][-1]['content'])['editable_text_units']))
  text='';offset=0
  while True:
   p=self.op('memory.page',{'kind':'communication','object_id':out['cut_id'],'offset':offset,'limit':43});text+=p['value']
   if p['next_offset'] is None:break
   offset=p['next_offset']
  self.assertEqual(json.loads(text)['communications'][0]['message_id'],'message1')
 def test_spawn_receipt_only_p_and_v(self):
  for life in ('P','V'):
   out=self.op('socket.spawn',{'socket_id':self.sock,'text':'initial peer question','message_id':'spawn-'+life,'lifecycle':life});state=self.core.store.state(out['chain_id']);self.assertEqual(state['inbox'],[]);self.assertEqual(len([e for e in state['events'] if e.get('type')=='communication_received']),1)
   sid=next(sid for group in state['groups'].values() for sid in group if communication.source_provenance(self.core.store.get(sid,'source')));row=communication.source_provenance(self.core.store.get(sid,'source'))[0];self.assertTrue(row['received_at']);self.assertEqual(row['checkpoint_id'],self.cp)
 def test_conflicts_permissions_and_metadata_protection(self):
  self.send();before=self.op('chain.get')
  with self.assertRaises(CoreError):self.send(text='changed')
  self.assertEqual(before,self.op('chain.get'))
  other=self.op('chain.create')['chain_id'];old=self.ctx['chain_id'];self.ctx['chain_id']=other
  with self.assertRaises(CoreError):self.op('memory.page',{'kind':'communication','object_id':before['current_cut']})
  self.ctx['chain_id']=old
  with self.core.store.transaction():
   with self.assertRaises(CoreError):communication.receive_message(self.core,self.core.store.state(old),{'message_id':'spoof','text':'t','lifecycle':'V','received_at':1})
 def test_concurrent_distinct_and_duplicate_arrivals(self):
  with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:results=list(pool.map(lambda _:self.send(),range(8)))
  self.assertTrue(all(x==results[0] for x in results));self.assertEqual(len(self.op('chain.get')['inbox']),1)
  with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:list(pool.map(lambda i:self.send('ordered'+str(i)),range(8)))
  state=self.op('chain.get');messages=state['inbox'];self.assertEqual([x['received_at'] for x in messages],sorted(x['received_at'] for x in messages));self.assertEqual(len({x['received_event_id'] for x in messages}),9)
 def test_authenticated_http_receipt_and_revoked_history(self):
  with patch.dict(os.environ,{'SIC_COMM_TEST_TOKEN':'isolated-unit-token'}):server=make_server(self.core,{'tokens':{'SIC_COMM_TEST_TOKEN':'host'}},port=0)
  thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
  def http(n,p,ctx=None):
   body=json.dumps({'operation':n,'context':ctx or self.ctx,'payload':p}).encode();req=urllib.request.Request('http://127.0.0.1:'+str(server.server_port)+'/api/v2/operations',body,{'Authorization':'Bearer isolated-unit-token','Content-Type':'application/json'})
   with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req,timeout=5) as response:return json.load(response)['result']
  try:
   a=http('socket.message',{'socket_id':self.sock,'message_id':'http','text':'HTTP evidence','lifecycle':'P'});self.assertEqual(a,http('socket.message',{'socket_id':self.sock,'message_id':'http','text':'HTTP evidence','lifecycle':'P'}));out=self.publish(self.raw('after receive'));row=http('memory.page',{'kind':'communication','object_id':out['cut_id']});self.assertIn(a['receive_receipt_id'],row['value'])
   with self.assertRaises(urllib.error.HTTPError) as error:http('memory.page',{'kind':'communication','object_id':out['cut_id']},{**self.ctx,'actor':'attacker'})
   self.assertEqual(error.exception.code,403)
   sid=self.op('chain.get')['groups'];sid=next(iter(sid));self.op('security.revoke',{'source_ids':[sid],'reason':'independent-host','confirm_revoke':True})
   with self.assertRaises(urllib.error.HTTPError) as error:http('memory.page',{'kind':'communication','object_id':out['cut_id']})
   self.assertEqual(error.exception.code,403)
  finally:server.shutdown();server.server_close();thread.join(2)
 def test_private_publication_does_not_consume_worker_mail(self):
  self.send();self.op('hierarchy.context.create',{'context_id':'editor-notes'});f=self.op('hierarchy.context.ingest',{'context_id':'editor-notes','request':{'messages':[{'role':'user','content':'private editor material'}]}});self.publish(f['source_id']);self.assertEqual(self.op('chain.get')['delivered'],[]);self.assertNotIn('new withheld communication',json.dumps(self.request()))
 def test_large_metadata_uses_bounded_navigation_and_complete_page(self):
  for i in range(30):self.send('bulk-'+str(i),text='text-'+str(i))
  out=self.publish(self.raw('publish bulk'));self.op('config.update',{'memory_chars':1024});view=self.core._working_view(self.core.store.state(self.ctx['chain_id']),'worker');self.assertEqual(view['instant']['communication_page'],out['cut_id']);self.assertNotIn('received_at',json.dumps(view['instant']));text='';offset=0
  while True:
   page=self.op('memory.page',{'kind':'communication','object_id':out['cut_id'],'offset':offset,'limit':512});text+=page['value']
   if page['next_offset'] is None:break
   offset=page['next_offset']
  self.assertEqual(len(json.loads(text)['communications']),30)
 def upper(self,level,members):
  formed=self.op('hierarchy.form',{'level':level,'cb_ids':members});return self.publish(formed['source_id'])
 def test_active_third_level_receives_second_level_p_v_without_duplicate_members(self):
  first=self.core.store.state(self.ctx['chain_id'])['cbs'][0];second=self.publish(self.raw('base two'))['cb_id'];a=self.upper(2,[first,second]);more=[self.publish(self.raw('base '+str(i)))['cb_id'] for i in range(2)];b=self.upper(2,more);self.upper(3,[a['cb_id'],b['cb_id']]);self.op('hierarchy.select',{'level':3,'roles':['worker']})
  tail=[self.publish(self.raw('tail '+str(i)))['cb_id'] for i in range(2)];f=self.op('hierarchy.form',{'level':2,'cb_ids':tail});before=self.request();self.send('upperP','P','upper permanent delivery');self.send('upperV','V','upper next WB delivery');self.assertEqual(before,self.request());out=self.publish(f['source_id']);state=self.op('chain.get');self.assertEqual(state['delivered'],['upperP','upperV']);self.assertIn(out['cb_id'],__import__('sic_core.hierarchy',fromlist=['selected_ids']).selected_ids(self.core,state,'worker'));req=self.request();self.assertIn('upper permanent delivery',json.dumps(req));self.assertIn('upper next WB delivery',json.dumps(req));self.assertEqual(sum(m.get('content')=='upper next WB delivery' for m in req['messages']),1);self.assertEqual(self.core.store.get(state['hierarchy']['levels']['2']['wbs'][-1],'wb')['source_id'],out['volatile_source_id'])
  sources=communication.pending_sources(self.core,state);self.assertIn(out['volatile_source_id'],sources);self.assertNotIn(f['source_id'],sources)
  self.publish(out['volatile_source_id']);self.assertEqual(self.op('chain.get')['delivered'],['upperP','upperV'])
 def test_inactive_higher_publication_defers_until_visible_window(self):
  first=self.core.store.state(self.ctx['chain_id'])['cbs'][0];second=self.publish(self.raw('second'))['cb_id'];f=self.op('hierarchy.form',{'level':2,'cb_ids':[first,second]});self.send();out=self.publish(f['source_id']);self.assertEqual(self.op('chain.get')['delivered'],[]);self.assertNotIn('new withheld communication',json.dumps(self.request()));self.assertNotIn('communication',json.dumps(self.core.store.get(out['cb_id'],'cb')['ops']));self.publish(self.raw('new visible lower tail'));self.assertEqual(self.op('chain.get')['delivered'],['message1']);self.assertIn('new withheld communication',json.dumps(self.request()))
 def test_worker_explicit_internal_context_receives_only_its_visible_window(self):
  self.op('hierarchy.context.create',{'context_id':'private-worker'});f=self.op('hierarchy.context.ingest',{'context_id':'private-worker','request':{'messages':[{'role':'user','content':'private work'}]}});self.publish(f['source_id']);self.op('hierarchy.select',{'context_id':'private-worker','level':1,'roles':['worker']});self.send(life='V');self.publish(self.raw('unrelated main publication'));self.assertEqual(self.op('chain.get')['delivered'],[])
  f=self.op('hierarchy.context.ingest',{'context_id':'private-worker','request':{'messages':[{'role':'user','content':'actual recipient work'}]}});out=self.publish(f['source_id']);self.assertEqual(self.op('chain.get')['delivered'],['message1']);self.assertIn('new withheld communication',json.dumps(self.request()));self.assertIn(out['volatile_source_id'],communication.pending_sources(self.core,self.op('chain.get')))
 def test_notes_background_keeps_main_native_work_and_media(self):
  from test_core_v2_media import image
  self.op('hierarchy.context.create',{'context_id':'notes'});f=self.op('hierarchy.context.ingest',{'context_id':'notes','request':{'messages':[{'role':'user','content':'notes background'}]}});self.publish(f['source_id']);self.op('hierarchy.select',{'context_id':'notes','level':1,'roles':['worker']});block=image(size=(8,8));native=[{'role':'user','content':[{'type':'text','text':'main real new task'},block]},{'role':'assistant','content':'main progress','tool_calls':[{'id':'actual-call','type':'function','function':{'name':'read_file','arguments':'{}'}}]},{'role':'tool','tool_call_id':'actual-call','content':'actual native tool result'}];sid=self.op('source.ingest',{'request':{'messages':native}})['source_id'];request=self.request();self.assertEqual(request['messages'][-3:],native);self.assertIn(sid,communication.pending_sources(self.core,self.op('chain.get')))
  other=self.op('hierarchy.context.create',{'context_id':'unshared'});f=self.op('hierarchy.context.ingest',{'context_id':'unshared','request':{'messages':[{'role':'user','content':'UNSHARED_EDITOR_PRIVATE'}]}});self.assertNotIn('UNSHARED_EDITOR_PRIVATE',json.dumps(self.request()));self.assertEqual(self.request()['messages'][-3:],native)
if __name__=='__main__':unittest.main()

