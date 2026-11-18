// Create an isolated copy of the actual DSH direct Messages adapter, adding a
// single optional Provider-owned fetch seam. No installed/source file changes.
import {readFile,mkdir,writeFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';

export async function prepareDeepSeekFixture(dshRoot,outputRoot){
  const packageRoot=join(dshRoot,'packages/llm/llm-deepseek');
  const source=join(packageRoot,'lib/index.js');
  const raw=await readFile(source,'utf8');
  const needle='const response = await fetch(`${messagesApiRoot(connection.baseURL)}/messages`, {';
  if(raw.split(needle).length!==2)throw Error('unrecognized actual DeepSeek native transport source');
  let modified=raw.replace(needle,'const response = await (this.dependencies.fetch ?? fetch)(`${messagesApiRoot(connection.baseURL)}/messages`, {');
  const close='\n\t\t\t\t});\n\t\t\t\tif (!response.ok)';
  if(modified.split(close).length!==2)throw Error('unrecognized actual DeepSeek native fetch close');
  modified=modified.replace(close,'\n\t\t\t\t}, { options, history, context_projection: contextProjection?.projection, serializeHistory });\n\t\t\t\tif (!response.ok)');
  const serialization='const body = serialize(options, connection, inline ? inlineImages(messages, versions, connection) : messages, versions,';
  if(modified.split(serialization).length!==2)throw Error('unrecognized actual serializer input');
  modified=modified.replace(serialization,'const originalHistory = inline ? inlineImages(messages, versions, connection) : messages;\n const contextProjection = await this.dependencies.projectContext?.(originalHistory, options);\n const history = contextProjection?.history ?? originalHistory;\n const serializeHistory = history => serialize(options, connection, history, versions,');
  const endSerialize='}, fileIds);';
  if(modified.split(endSerialize).length!==2)throw Error('unrecognized actual serializer close');
  modified=modified.replace(endSerialize,'}, fileIds);\n const body = serializeHistory(history);');
  const require=createRequire(source);
  modified=modified.replace(/from "([^"\n]+)"/g,(all,spec)=>spec.startsWith('node:')?all:`from ${JSON.stringify(pathToFileURL(require.resolve(spec)).href)}`);
  await mkdir(outputRoot,{recursive:true});
  const output=join(outputRoot,'dsh-deepseek-native-fetch.mjs');await writeFile(output,modified,'utf8');
  await writeFile(join(outputRoot,'manifest.json'),JSON.stringify({source,source_sha256:createHash('sha256').update(raw).digest('hex'),
    output,patch:'optional dependencies.fetch at existing native Messages HTTP dispatch; static imports rebound to original dependency resolution'},null,2));
  return output;
}
