import argparse
from pathlib import Path
import difflib,json
parser=argparse.ArgumentParser()
parser.add_argument('--dsh-root',type=Path,required=True)
parser.add_argument('--fixture-root',type=Path,required=True)
args=parser.parse_args()
root=Path(__file__).resolve().parent
dsh=args.dsh_root.resolve();base='packages/llm/llm-deepseek/src/';changes={}
def replace(name,old,new):
 text=changes.get(name,(dsh/base/name).read_text(encoding='utf-8'))
 if text.count(old)!=1:raise RuntimeError(f'unreviewed DSH source: {name}')
 changes[name]=text.replace(old,new)
replace('types.ts','ImageAttachmentAccess }','ImageAttachmentAccess, GenerateOptions, RequestMessage }')
replace('types.ts','  /** Report unusable native Messages replay metadata',"""  /** Host SDK projection before the original Provider serializer. */
  projectContext?: (history: readonly RequestMessage[], options: GenerateOptions) => Promise<{history: readonly RequestMessage[]; projection?: unknown}>
  /** Native HTTP hook with host-only provenance. */
  fetch?: (input: RequestInfo | URL, init: RequestInit, provenance: {options: GenerateOptions; history: readonly RequestMessage[]; context_projection?: unknown; serializeHistory: (history: readonly RequestMessage[]) => unknown}) => Promise<Response>
  /** Report unusable native Messages replay metadata""")
replace('adapter.ts','GenerateOptions, ImageAttachmentAccessResolver', 'GenerateOptions, RequestMessage, ImageAttachmentAccessResolver')
replace('adapter.ts','const body = serialize(options, connection, history, versions,', 'const serializeHistory = (history: readonly RequestMessage[]) => serialize(options, connection, history, versions,')
replace('adapter.ts','        }, fileIds)\n        const extensions', '        }, fileIds)\n        const body = serializeHistory(history)\n        const extensions')
replace('adapter.ts','const history = inline ? inlineImages(messages, versions, connection) : messages',"""const originalHistory = inline ? inlineImages(messages, versions, connection) : messages
        const contextProjection = await this.dependencies.projectContext?.(originalHistory, options)
        const history = contextProjection?.history ?? originalHistory""")
replace('adapter.ts','const response = await fetch(`${messagesApiRoot(connection.baseURL)}/messages`, {','const nativeFetch = this.dependencies.fetch ?? ((input: RequestInfo | URL, init: RequestInit, _provenance: unknown) => fetch(input, init))\n        const response = await nativeFetch(`${messagesApiRoot(connection.baseURL)}/messages`, {')
replace('adapter.ts','        })\n        if (!response.ok)','        }, {options, history, context_projection: contextProjection?.projection, serializeHistory})\n        if (!response.ok)')
replace('host.ts',"import { resolveImageAttachmentAccess } from '@deepseek-ai/dsh-llm'","import { resolveImageAttachmentAccess } from '@deepseek-ai/dsh-llm'\nimport type { GenerateOptions, RequestMessage } from '@deepseek-ai/dsh-llm'")
replace('host.ts','    ...dependencies,',"""    ...dependencies,
    projectContext: async (history, options) => {
      const exchange = {history, options, projection: undefined as unknown}
      const projected = await ctx.waterfall('llm/context-project', exchange, () => Promise.resolve(exchange.history))
      return {history: projected, projection: exchange.projection}
    },
    fetch: (input, init, provenance) => {
      const guards: Array<(request: Request) => void | Promise<void>> = []
      const exchange = {request: new Request(input, init), provenance, format: 'anthropic-messages' as const} as NativeFetchExchange
      Object.defineProperty(exchange, 'addFinalGuard', {value: (guard: (request: Request) => void | Promise<void>) => {guards.push(guard)}, writable: false, configurable: false})
      Object.preventExtensions(exchange)
      return ctx.waterfall('llm/native-fetch', exchange, async () => {
        for (const guard of guards) await guard(exchange.request)
        return fetch(exchange.request)
      })
    },""")
changes['host.ts']+="""
interface NativeFetchExchange {
  request: Request
  readonly provenance: Parameters<NonNullable<DeepSeekAdapterOptions['fetch']>>[2]
  readonly format: 'anthropic-messages'
  readonly addFinalGuard: (guard: (request: Request) => void | Promise<void>) => void
}
declare module '@deepseek-ai/cordis' {
  interface Events {
    'llm/context-project'(exchange: {history: readonly RequestMessage[]; options: GenerateOptions; projection?: unknown}, next: () => Promise<readonly RequestMessage[]>): Promise<readonly RequestMessage[]>
    'llm/native-fetch'(exchange: NativeFetchExchange, next: () => Promise<Response>): Promise<Response>
  }
}
"""
patch=''
for name,new in changes.items():
 old=(dsh/base/name).read_text(encoding='utf-8')
 patch+=''.join(difflib.unified_diff(old.splitlines(True),new.splitlines(True),fromfile='a/'+base+name,tofile='b/'+base+name))
patch=patch.replace('\n \n','\n\n')
(root/'deepseek-native-fetch.patch').write_text(patch,encoding='utf-8',newline='\n')
out=args.fixture_root.resolve();out.mkdir(parents=True,exist_ok=True)
(out/'typescript-overlay.json').write_text(json.dumps({str(dsh/base/n):v for n,v in changes.items()},ensure_ascii=False),encoding='utf-8')
print('Generated reviewed context projection and monotonic native transport patch.')
