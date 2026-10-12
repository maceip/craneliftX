Binary Factory renderer — designer delivery dated 2026-10-12 13:28.
Derived from:
/Users/mac/Downloads/animation-blocks/deliverables/factory/renderer/factory-renderer.js
The nine scenes reuse the original 45-degree artwork and five machines.
Host adapter: webapp/src/components/FactoryBackdrop.tsx
The renderer includes Three.js; its MIT license is in THREE-LICENSE.txt.
The editable source and original distribution remain in animation-blocks/deliverables.
Original delivery SHA-256: ed415d324dd4006c66ad721e059099cb9a6c4263e41ab163f75b709dedd91f7b

The host selects quality: "backdrop": a 24fps direct render at a maximum 1x
pixel ratio, without the standalone bloom, multisample and shadow passes.
Geometry, materials, 45-degree camera, mechanical animation and result logic
are unchanged. Omitting this option retains the full-quality standalone path.
This keeps the foreground UI responsive on software-rendered WebGL devices.
