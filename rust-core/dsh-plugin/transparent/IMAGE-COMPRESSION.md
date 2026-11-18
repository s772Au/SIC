# Native image compression and lineage

Enable `"imageCompression": true` in the transparent plugin's SDK edit
configuration. Start the native DSH attachment service first and make Sharp
available to the plugin loader as an optional peer. JSON/YAML and the advanced
factory use the same implementation. The existing SDK carries the resulting
attachment through the original Provider adapter.

An Editor receives the coordinates and identities of authorized native image
occurrences. Its ordinary `sic_edit_blocks` delivery can set
`pvi.I["sic-image-compression-v1"]` to a recipe.

```json
{
  "crops": [{"message_id":"native-message-id","block_index":1,"x":10,"y":20,"width":240,"height":120}],
  "description": [{"text":"A status panel shows the item is pending.","tiles":[1]}],
  "display": "mosaic"
}
```

Coordinates refer to the immutable raster already admitted by DSH. The host
checks every region against that original, saves the crop through the native
attachment store and makes a compact mosaic. Crops fit into tiles without
enlargement or aspect-ratio changes. The model selects regions and writes the
description. The host chooses the deterministic arrangement and validates
citations. The recipe is limited to 12 crops and 8 million total crop pixels.

The persistent links are original occurrence → crop pixels and original
coordinates → mosaic pixel placement → description with tile citations. Each
asset has its own identity, and attachment bytes are checked against the
native content digest. Descriptions are derived observations. They neither
change the original evidence nor grant permissions.

Crop coordinates are exact in the admitted source raster. Native attachment
normalization and request encoding may change decoded pixel values in an
archived derivative without changing its geometry. Inspect the original when
a task requires pixel fidelity beyond that derivative.

`mosaic` is the default display and sends an actual image in the I message.
`crops` sends separate crop images. `description` sends the cited text with
zero image blocks for those origins. Omitting the I key leaves the view in
place. Setting it to `null` restores the native full-image projection. New
media outside the compressed group retains its native representation.

The host stores the verified lineage before submitting its manifest identity
and digest in the ordinary Core I transaction. The exact Core text-I receipt
continues to attest the named view. The durable SDK layout receipt records
the actual image representation alongside its selected cut. A new view
applies to subsequent requests. A saved request reuses its own snapshot and
does not regenerate pixels or prompt the Editor on replay. Published CBs and
original Session events remain untouched. If a native source becomes
offloaded or unavailable in the current request, the host suppresses its old
derivatives and follows the original native projection.

The Worker can call `sic_image_source` for one predecessor of the active
view. Use the manifest shown in I and pass `from: "description"` for its
mosaic, `from: "mosaic"` with a tile for the crop, or `from: "crop"` with
that tile for its original. A predecessor must have been sent in the current
view or returned by a verified native lookup in the request history. The
host verifies the admitted Worker call, exact Session boundary, Core access,
cut and attachment bytes before returning a native image result. Previously
returned derivatives no longer contribute pixels when their source is
offloaded or absent. Disabling image compression restores native input image
projection on subsequent requests and leaves archived lineage intact.

Use the SIC context view in DSH to select a saved request and inspect its
I image lineage. Description citations identify mosaic tiles. Each tile can
open its crop and original through the authorized projection endpoint. Binary
reads are scoped to that projection's manifest and existing Session/Core
access checks. Keep the host projection directory, native Session, native
attachment store, trusted bindings and Core database in the same backup set.

This feature applies to the SDK edit path. A first request containing only an
image passes through the existing read-only path. Image recipes can be created
when an authorized Editor handles a subsequent working batch. Archive-only
objects and offloaded pictures cannot be introduced as new crop sources.
