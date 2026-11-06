# Freeform Text — editable character transforms

This is a **native PhotoCraft Type-layer** feature in the
`feature/freeform-text-20261106` branch. It does not rasterise letters,
convert text to smart objects, or create a separate layer for each character.

## How to use

1. Select a Type layer and activate the regular **Type tool**.
2. Enable **◎ Freeform Text** in the Type options bar.
3. For each character or grapheme cluster:
   - Drag the **centre dot** to move it freely.
   - Drag the **ring** to rotate it.
   - Drag the **square above-right** to scale it.
4. Hold **Shift** while rotating to snap to 15-degree steps.
5. **Alt-click** a handle to reset that character's freeform adjustments.
6. Turn off the mode to edit the text normally. Switching it on again
   shows the stored controls in their current positions.

Undo/redo coalesces each drag as a single Type edit.

## Storage and text changes

Offsets (text-space X/Y pixels), angle (degrees) and scale (factor) are
**non-destructive properties of the character-style runs**. Type commands
address them by Unicode character range; the model stores UTF-8 byte
ranges so Cyrillic and variable-length Unicode sequences stay aligned.
A grapheme cluster is transformed as one unit.

When text is edited, unchanged characters keep their runs and freeform
transforms. Replacing an existing character inherits the first replaced
character's placement; genuinely inserted characters inherit typography
but start without any freeform adjustment. Removing characters removes
their run spans. Native **.pcraft** documents store the settings with
the text layer.

Freeform scaling is deliberately independent of font size, so resizing
a glyph does not reflow its paragraph. Text Library effects and manual
freeform transforms can coexist.

## Current limitations

- This is a native PhotoCraft extension: saving as PSD may not preserve
  the manually transformed glyph appearance as editable Type data.
- The mode currently edits one grapheme cluster at a time; batch
  selection and group transforms can be added later.
- The controls track the normal glyph layout and are most precise on
  point/paragraph text. Text-on-path and other non-linear warps can
  require special handling.
- Successful Windows builds must be confirmed by the branch's GitHub
  Actions workflow; source changes alone do not prove the build passes.
