"""Frozen non-LLM edit controls. No network, sampling, or implicit model loading."""
from importlib.metadata import version

def unit_ops(units,engine='program_exact',budget=4096):
    if engine=='llmlingua':
        from scripts.llmlingua_adapter import compress_units
        return compress_units(units)['ops']
    if engine=='program_exact':
        from sic_core.editing import program_ops
        return program_ops({'units':units},['deduplicate_exact','drop_empty','normalize_newlines'])
    if engine=='unchanged':return []
    if engine=='langchain_trim':
        if version('langchain-core')!='1.7.6':raise ValueError('Frozen langchain-core==1.7.6 required')
        from langchain_core.messages import HumanMessage,trim_messages
        messages=[HumanMessage(content=u['text'],id=u['unit_id']) for u in units]
        kept=trim_messages(messages,max_tokens=budget,token_counter=lambda ms:sum(len(m.content) for m in ms),strategy='last',allow_partial=False)
        ids={m.id for m in kept}
        return [{'op':'DELETE','target':u['unit_id'],'reason':'Frozen whole-text-unit last strategy; unicode-codepoint budget, not billed tokens'} for u in units if u['unit_id'] not in ids]
    raise ValueError('Unknown non-LLM control; missing engines never silently fall back')
