import { createHash } from "node:crypto";
import { access } from "node:fs/promises";
import { isAbsolute } from "node:path";
import { pathToFileURL } from "node:url";
import type { ImageContent, TextContent } from "@earendil-works/pi-ai";
import type { ExtensionAPI, SessionEntry } from "@earendil-works/pi-coding-agent";

export type ImageRef = { entry: string; block: number; digest: string };
export type Pointer = {
  manifest_id: string;
  lineage_digest: string;
  display: "description" | "mosaic" | "crops";
};
export type RecipeSource = { message_id: string; block_index: number; width: number; height: number };
type ImageWithOrigin = { ref: ImageRef; image: ImageContent };
type Asset = { attachmentId: string; mediaType: string; width: number; height: number };
type ImageAsset = { id: string; attachment: Asset };
type Choice = { kind: "mosaic" | "crop" | "original"; item: ImageAsset; tile?: number };
type Ref = { message_id: string; block_index: number };
type Lineage = {
  originals: (Ref & ImageAsset)[];
  crops: (ImageAsset & { parent_id: string })[];
  mosaic: ImageAsset & { tiles: { number: number; crop_id: string; placement: { x: number; y: number; width: number; height: number } }[] };
  description: { parts: { text: string; tiles: number[] }[] };
};
type View = { pointer: Pointer; lineage: Lineage; active: boolean };
type Lookup = { manifest_id: string; from: "description" | "mosaic" | "crop"; tile?: number };
type VisualEntry = { manifests: [string, unknown][]; assets: [string, string][] };
type Carrier = { id: string; role: "user"; content: ({ type: "image"; attachment: Asset } | { type: "slot" })[] };
type Codec = {
  offer(history: Carrier[]): RecipeSource[];
  prepare(args: { args: { pvi: { I: Record<string, unknown> } }; history: Carrier[]; sessionId: string;
    signal?: AbortSignal }): Promise<{ pvi: { I: Record<string, Pointer> } }>;
  project(args: { history: Carrier[]; sessionId: string; pointer: Pointer;
    signal?: AbortSignal }): Promise<View>;
  readAsset(session: string, pointer: Pointer, id: string, signal?: AbortSignal): Promise<{ data: Uint8Array; mediaType: string } | null>;
};

export const visualTool = "sic_image_source";
export const imageContract = "sic-image-compression-v1";
const journalEntry = "sic-pi-image-lineage-v1";
const resultMarker = "SIC previous image representation ";
const bytesHash = (data: Uint8Array) => "sha256:" + createHash("sha256").update(data).digest("hex");
const position = (entry: string, block: number) => JSON.stringify([entry, block]);

export async function openVisual(sharpPath: string) {
  if (!isAbsolute(sharpPath)) throw new Error("Pi visual compression requires the absolute path to Sharp");
  const sourceMedia = new URL("../../rust-core/dsh-plugin/transparent/", import.meta.url);
  const mediaBase = await access(new URL("image-compression.mjs", sourceMedia))
    .then(() => sourceMedia, () => new URL("../../dsh-plugin/", import.meta.url));
  const [compression, source, { default: sharp }] = await Promise.all([
    import(new URL("image-compression.mjs", mediaBase).href),
    import(new URL("image-source-tool.mjs", mediaBase).href),
    import(pathToFileURL(sharpPath).href),
  ]);
  const choose = source.imagePredecessor as (view: View, args: Lookup) => Choice;

  async function open(session: string, branch: SessionEntry[], originals: ImageWithOrigin[], signal?: AbortSignal) {
    const records = new Map<string, unknown>();
    const stored = new Map<string, string>();
    const manifests = new Map<string, unknown>();
    const assets = new Map<string, string>();
    const inputs = new Map<string, Buffer>();
    const messages = new Map<string, Carrier>();
    for (const row of branch) {
      if (row.type !== "custom" || row.customType !== journalEntry) continue;
      const entry = row.data as VisualEntry;
      for (const [key, value] of entry.manifests) records.set(key, value);
      for (const [key, value] of entry.assets) stored.set(key, value);
    }
    for (const { ref, image } of originals) {
      signal?.throwIfAborted();
      const data = Buffer.from(image.data, "base64");
      const hash = bytesHash(data);
      const metadata = await sharp(data, { failOn: "error", limitInputPixels: 64_000_000 }).metadata();
      const mime = { jpeg: "image/jpeg", png: "image/png", gif: "image/gif", webp: "image/webp" }[
        metadata.format as "jpeg" | "png" | "gif" | "webp"];
      if (!mime || mime !== image.mimeType || (metadata.pages ?? 1) !== 1 || !metadata.width || !metadata.height
          || metadata.width * metadata.height > 64_000_000 || (metadata.orientation ?? 1) !== 1) {
        throw new Error("Pi visual compression requires a correctly typed, single-frame raster");
      }
      inputs.set(hash, data);
      const entry = messages.get(ref.entry) ?? { id: ref.entry, role: "user", content: [] };
      while (entry.content.length <= ref.block) entry.content.push({ type: "slot" });
      entry.content[ref.block] = { type: "image", attachment: { attachmentId: hash, mediaType: mime,
        width: metadata.width, height: metadata.height } };
      messages.set(ref.entry, entry);
    }
    const codec = compression.createImageCompression({
      sharp,
      store: { async context(id: string, key: string, build: () => Promise<unknown>) {
        if (id !== session) throw new Error("Pi visual journal belongs to a different session");
        if (records.has(key)) return records.get(key);
        if (manifests.has(key)) return manifests.get(key);
        const value = await build();
        manifests.set(key, value);
        return value;
      } },
      attachments: {
        async readImage(ref: Asset) {
          const data = inputs.get(ref.attachmentId);
          if (data) return { data };
          const storedBytes = assets.get(ref.attachmentId) ?? stored.get(ref.attachmentId);
          if (!storedBytes) throw new Error("Pi visual asset is absent from the session journal");
          return { data: Buffer.from(storedBytes, "base64") };
        },
        async saveImage({ data, mediaType }: { data: Buffer; mediaType: string }) {
          const metadata = await sharp(data, { failOn: "error", limitInputPixels: 64_000_000 }).metadata();
          const attachmentId = bytesHash(data);
          if (!inputs.has(attachmentId) && !stored.has(attachmentId)) assets.set(attachmentId, data.toString("base64"));
          return { attachmentId, mediaType, width: metadata.width, height: metadata.height };
        },
      },
    }) as Codec;
    const history = [...messages.values()];
    const project = async (pointer: Pointer) => {
      const view = await codec.project({ sessionId: session, history, pointer, signal });
      if (!view.active) throw new Error("Pi visual lineage no longer matches the original images");
      return view;
    };
    async function image(pointer: Pointer, id: string): Promise<ImageContent> {
      const candidate = await codec.readAsset(session, pointer, id, signal);
      if (!candidate) throw new Error("Pi visual asset is outside the authorized lineage");
      return { type: "image", data: Buffer.from(candidate.data).toString("base64"), mimeType: candidate.mediaType };
    }
    return {
      sources: codec.offer(history),
      async prepare(recipe: unknown, pi: ExtensionAPI) {
        const response = await codec.prepare({ args: { pvi: { I: { [imageContract]: recipe } } }, sessionId: session, history, signal });
        const pointer = response.pvi.I[imageContract];
        if (manifests.size || assets.size) pi.appendEntry(journalEntry, { manifests: [...manifests], assets: [...assets] });
        return pointer;
      },
      async render(pointer: Pointer) {
        const view = await project(pointer);
        const { lineage } = view;
        const text = { type: "text" as const, text: JSON.stringify({
          sic_visual_evidence: { manifest_id: pointer.manifest_id, representation: pointer.display,
            ...(pointer.display === "description" ? { description: lineage.description.parts } : {}),
            tiles: lineage.mosaic.tiles.map(({ number, placement }) => ({ number,
              ...(pointer.display === "mosaic" ? { placement } : {}) })) },
        }) };
        const content: (ImageContent | TextContent)[] = [text];
        if (pointer.display === "mosaic") content.push(await image(pointer, lineage.mosaic.id));
        if (pointer.display === "crops") {
          for (const crop of lineage.crops) content.push(await image(pointer, crop.id));
        }
        return { content, covered: new Set(lineage.originals.map(ref => position(ref.message_id, ref.block_index))) };
      },
      async lookup(pointer: Pointer, args: Lookup, keptBranch: SessionEntry[]) {
        const view = await project(pointer);
        const selected = choose(view, args);
        const tile = view.lineage.mosaic.tiles.find(row => row.number === args.tile);
        const prior = args.from === "crop" ? view.lineage.crops.find(row => row.id === tile?.crop_id) : view.lineage.mosaic;
        const allowed = args.from === "description" ? pointer.display === "description"
          : args.from === "mosaic" ? pointer.display === "mosaic"
          : pointer.display === "crops";
        const proof = prior && keptBranch.some(row => {
          if (row.type !== "message" || row.message.role !== "toolResult" || row.message.isError
              || row.message.toolName !== visualTool) return false;
          const [note, asset] = row.message.content;
          return note?.type === "text" && note.text === resultMarker + JSON.stringify({
            manifest_id: pointer.manifest_id, asset_id: prior.id,
          }) && asset?.type === "image" && bytesHash(Buffer.from(asset.data, "base64")) === prior.attachment.attachmentId;
        });
        if (!allowed && (args.from === "description" || !proof)) throw new Error("Inspect the current representation before stepping back");
        const note = resultMarker + JSON.stringify({ manifest_id: pointer.manifest_id, asset_id: selected.item.id });
        return { content: [{ type: "text" as const, text: note }, await image(pointer, selected.item.id)], details: {} };
      },
    };
  }
  return open;
}

export const imagePosition = (ref: ImageRef) => position(ref.entry, ref.block);
