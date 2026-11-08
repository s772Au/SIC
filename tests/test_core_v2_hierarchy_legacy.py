"""Old immutable archives stay accessible without invented WB chronology."""
import copy,json,unittest
from sic_core import Core,CoreError
from sic_core.adapters import decode,digest
from sic_core.store import uid


class LegacyHierarchyTests(unittest.TestCase):
    def setUp(self):
        self.core=Core();self.context={'actor':'host','allow_write':True,'allow_model':False};self.context['chain_id']=self.op('chain.create')['chain_id']
    def tearDown(self):self.core.close()
    def op(self,name,p=None,context=None):return self.core.operate(name,context or self.context,p or {})['result']
    def old(self,text):
        with self.core.store.transaction():
            state=self.core.store.state(self.context['chain_id']);state.pop('hierarchy',None);state['tools']=[t for t in state['tools'] if not t['name'].startswith('sic_hierarchy_') and t['name']!='sic_memory_replay']
            source=decode('openai-chat',{'messages':[{'role':'user','content':text}]});sid=self.core.store.put('source',source);wid=self.core.store.put('wb',{'chain_id':state['chain_id'],'source_ids':[sid],'unit_ids':['u1'],'reason':'publication-membership','job_id':'legacy-job'})
            cid=uid('cb');cb={'cb_id':cid,'chain_id':state['chain_id'],'sequence':len(state['cbs'])+1,'source_id':sid,'source_ids':[sid],'body':text,'ops':[{'op':'KEEP','targets':['u1'],'origin':'implicit-program'}],'index':[],'job_id':'legacy-job','wb_id':wid,'protocol_events':[]};self.core.store.put('cb',cb,cid);state['cbs'].append(cid);state['groups'][sid]=[sid]
            cut={k:copy.deepcopy(state[k]) for k in ('chain_id','cbs','instant','pending','protected','groups','config','tools','tool_pages','board_id')};cut.update(cut_id=uid('cut'),sequence=state['cut_seq']+1,controls={});cut['digest']=digest(cut);self.core.store.put('cut',cut,cut['cut_id']);state['current_cut']=cut['cut_id'];state['cut_seq']=cut['sequence'];self.core.store.save(state)
        return {'source_id':sid,'wb_id':wid,'cb_id':cid}
    def newer(self):
        sid=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'new continuation'}]}})['source_id'];job=self.op('edit.program',{'source_id':sid,'rules':[]})['job_id'];return self.op('cut.publish',{'job_id':job,'expected_cut':self.op('chain.get')['cut_seq'],'idempotency_key':job})

    def test_old_and_new_actual_role_views_keep_legacy_with_bounded_explicit_navigation(self):
        legacy=self.old('legacy original detail');new=self.newer();state=self.op('chain.get');state['_actor']='host';worker=self.core._working_view(state,'worker');self.assertEqual([x['cb_id'] for x in worker['cbs']],[legacy['cb_id'],new['cb_id']]);self.assertEqual(worker['instant']['memory_projection']['legacy_unindexed_count'],1)
        sid=self.op('source.ingest',{'request':{'messages':[{'role':'user','content':'next editor input'}]}})['source_id'];job=self.op('edit.prepare',{'source_id':sid})['job'];editor=job['editor_backgrounds']['editor-1'];self.assertEqual([x['cb_id'] for x in editor['cbs']],[legacy['cb_id'],new['cb_id']]);self.assertEqual(editor['manifest']['legacy_unindexed_count'],1)
        for projection in (self.op('memory.project',{'role':'worker'}),editor):
            nav=projection['manifest']['legacy_unindexed_navigation'];self.assertEqual(nav['operation'],'hierarchy.legacy');self.assertEqual(nav['tool'],'sic_operation_directory');self.assertTrue(nav['fallback_directory_contains_all_cbs']);self.assertEqual(projection['cbs'][0]['level'],None);self.assertEqual(projection['cbs'][0]['wb_formation_order'],'unknown')
        rows=self.op('tools.invoke',{'name':'sic_operation_directory','arguments':{'offset':0,'limit':2}})['result']['rows'];self.assertEqual(rows[0]['hierarchy_index_status'],'legacy-unindexed');self.assertEqual(rows[0]['wb_formation_order'],'unknown');self.assertEqual(rows[1]['hierarchy_index_status'],'indexed');self.assertEqual(self.op('hierarchy.timeline',{'level':1,'kind':'wb'})['total'],2);self.assertNotIn(legacy['wb_id'],[row['wb_id'] for row in self.op('hierarchy.timeline',{'level':1,'kind':'wb'})['rows']]);self.assertEqual(self.op('usage.report')['calls'],[])

    def test_legacy_expand_both_directions_original_pages_replay_and_scope_guards(self):
        text='原始 legacy detail 🧭 preserved';old=self.old(text);self.newer();snapshot=list(self.core.store.db.iterdump())
        by_cb=self.op('hierarchy.expand',{'cb_id':old['cb_id']});by_wb=self.op('hierarchy.expand',{'wb_id':old['wb_id']});self.assertEqual(by_cb,by_wb);self.assertTrue(by_cb['legacy']);self.assertIsNone(by_cb['level']);self.assertEqual(by_cb['wb_formation_order'],'unknown');self.assertIsNone(by_cb['wb_sequence']);self.assertIsNone(by_cb['wb_formed_at']);self.assertEqual(by_cb['known_cb_order']['saved_sequence'],1);self.assertEqual(by_cb['original_units'][0]['text'],text)
        pieces=[];offset=0
        while True:
            page=self.op('memory.page',{**by_cb['complete_reads'][0],'offset':offset,'limit':7});pieces.append(page['value'])
            if page['next_offset'] is None:break
            offset=page['next_offset']
        self.assertEqual(''.join(pieces),text);replayed=self.op('memory.replay',{'cb_id':old['cb_id']});self.assertTrue(replayed['body_matches_published_cb']);self.assertEqual(replayed['body'],text);self.assertIsNone(replayed['level']);self.assertEqual(replayed['wb_formation_order'],'unknown');self.assertEqual(self.op('memory.locate',{'cb_id':old['cb_id']})['rows'][0]['wb_formation_order'],'unknown');self.assertEqual(snapshot,list(self.core.store.db.iterdump()))
        foreign=self.op('chain.create')['chain_id'];ctx={**self.context,'chain_id':foreign}
        for name,payload in [('hierarchy.expand',{'cb_id':old['cb_id']}),('hierarchy.expand',{'wb_id':old['wb_id']}),('memory.replay',{'cb_id':old['cb_id']}),('memory.page',{'kind':'source','object_id':old['source_id']})]:
            with self.assertRaises(CoreError):self.op(name,payload,ctx)
        self.assertEqual(self.op('hierarchy.legacy',{},ctx)['rows'],[])

    def test_legacy_directory_paginates_without_growing_projection_directory(self):
        first=self.old('one');small=self.op('memory.project',{'max_chars':512})['manifest']
        for i in range(100):self.old('legacy '+str(i))
        projected=self.op('memory.project',{'max_chars':512})['manifest'];self.assertEqual(projected['legacy_unindexed_count'],101);self.assertLess(len(json.dumps(projected))-len(json.dumps(small)),32);self.assertNotIn('legacy_unindexed_ids',projected)
        page=self.op('hierarchy.legacy',{'offset':0,'limit':2});self.assertEqual(page['legacy_unindexed_count'],101);self.assertEqual(len(page['rows']),2);self.assertEqual(page['rows'][0]['cb_id'],first['cb_id']);self.assertEqual(page['next_offset'],2);self.assertEqual(self.op('hierarchy.legacy',{'offset':100,'limit':2})['next_offset'],None)
        for payload in ({'offset':-1},{'limit':101},{'limit':0}):
            with self.assertRaises(CoreError):self.op('hierarchy.legacy',payload)

    def test_new_explicit_level_cannot_fall_back_when_required_wb_metadata_missing(self):
        with self.core.store.transaction():
            state=self.core.store.state(self.context['chain_id']);sid=self.core.store.put('source',decode('openai-chat',{'messages':[{'role':'user','content':'new'}]}));wid=self.core.store.put('wb',{'source_ids':[sid]});cid=uid('cb');self.core.store.put('cb',{'cb_id':cid,'chain_id':state['chain_id'],'level':1,'context_id':'main','sequence':1,'source_id':sid,'source_ids':[sid],'body':'new','ops':[],'index':[],'wb_id':wid},cid);state['cbs'].append(cid);state['groups'][sid]=[sid];state['hierarchy']['levels']['1']['wbs'].append(wid);state['hierarchy']['levels']['1']['pairs'][wid]=cid;self.core.store.save(state)
        with self.assertRaises(CoreError) as error:self.op('hierarchy.expand',{'cb_id':cid})
        self.assertEqual(error.exception.code,'hierarchy_integrity');self.assertEqual(self.op('memory.project')['manifest']['legacy_unindexed_count'],0)
