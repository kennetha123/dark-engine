# Runs inside Blender, driven by `dark-cli bake-model`. Never run by hand.
#
#   blender --background --factory-startup --python tools/bake_model.py -- \
#       <source> <mesh.glb> <baked.ron> <scale> <action>=<animation> ...
#
# Imports the artist's export, writes the glTF the view loads, and measures each clip. It emits
# RON directly rather than going through JSON so that a build tool needs no JSON dependency;
# `dark-cli` reads it straight back into `ModelMeasure`, so a wrong shape here fails loudly at
# the end of the bake rather than silently later.
#
# Times are reported in SECONDS. Turning them into simulation ticks is the engine's rule, and it
# lives in `dark_assets::model::ticks_for` so that a model and a Spine skeleton of the same
# duration agree.
import bpy, sys, os, math
from mathutils import Vector

argv = sys.argv[sys.argv.index("--") + 1:]
source, mesh_out, measure_out, scale = argv[0], argv[1], argv[2], float(argv[3])
wanted = []
for pair in argv[4:]:
    action, _, animation = pair.partition("=")
    wanted.append((action, animation))

bpy.ops.wm.read_factory_settings(use_empty=True)
lower = source.lower()
if lower.endswith(".fbx"):
    bpy.ops.import_scene.fbx(filepath=source)
elif lower.endswith((".glb", ".gltf")):
    bpy.ops.import_scene.gltf(filepath=source)
else:
    raise SystemExit(f"bake-model: {source} is not .fbx, .glb or .gltf")
bpy.context.view_layer.update()

meshes = [o for o in bpy.data.objects if o.type == 'MESH']
armatures = [o for o in bpy.data.objects if o.type == 'ARMATURE']
if not meshes:
    raise SystemExit(f"bake-model: {source} holds no mesh")

# World-space bounds. The object's own `dimensions` are the local mesh before the import's
# rotation, and a Mixamo FBX reads Y-up there while standing Z-up in the scene: measuring the
# world corners is the only answer that holds for every exporter.
lo = Vector((1e9, 1e9, 1e9))
hi = Vector((-1e9, -1e9, -1e9))
for ob in meshes:
    for corner in ob.bound_box:
        world = ob.matrix_world @ Vector(corner)
        lo = Vector(min(lo[i], world[i]) for i in range(3))
        hi = Vector(max(hi[i], world[i]) for i in range(3))
extent = hi - lo
# The vertical extent, not the largest one. Blender's importers put the model Z-up in world
# space whatever the source convention was, and `max()` would report a T-posed bind by its arm
# span, a quadruped by its length, and a prone body by how far it reaches. Nameplates and
# bubbles sit at this height, so it has to be the one over the head.
height = extent.z * scale

bones = max((len(a.data.bones) for a in armatures), default=0)
fps = bpy.context.scene.render.fps / max(bpy.context.scene.render.fps_base, 1e-6)

by_name = {a.name: a for a in bpy.data.actions}
clips, missing = [], []
for action, animation in wanted:
    act = by_name.get(animation)
    if act is None:
        missing.append(animation)
        continue
    first, last = act.frame_range
    seconds = max(last - first, 0.0) / fps
    # A marker on the action is an event: Blender's own way of saying "here", and what an
    # animator already reaches for. Times stay in seconds — turning them into ticks is the
    # engine's rule and lives in `dark_assets::model`, where a skeleton uses the same one.
    events = []
    for marker in act.pose_markers:
        events.append((marker.name, max(marker.frame - first, 0.0) / fps))
    events.sort(key=lambda e: (e[1], e[0]))
    clips.append((action, animation, seconds, events))

if missing:
    have = ", ".join(sorted(by_name)) or "none"
    raise SystemExit(
        f"bake-model: {source} has no animation named {', '.join(missing)}; it has {have}")

for path in (mesh_out, measure_out):
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
bpy.ops.export_scene.gltf(filepath=mesh_out, export_format='GLB', export_animations=True)

def ron_string(text):
    """A RON string literal. Animation names come from whatever an artist typed, and a quote or
    a backslash in one would otherwise write a file that does not parse."""
    escaped = text.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def ron_float(value):
    """RON wants a finite number. Blender can hand back a NaN bound box on a degenerate mesh."""
    return f"{value:.6f}" if math.isfinite(value) else "0.0"


# `dark-cli` turns this into the bake: it owns the tick rule, which clips loop, and the source
# hash. This file is the measurement only, and is deleted once it has been read.
with open(measure_out, "w", encoding="utf-8", newline="\n") as f:
    f.write("(\n")
    f.write(f"    height: {ron_float(height)},\n")
    f.write(f"    bones: {bones},\n")
    f.write("    clips: {\n")
    for action, animation, seconds, events in sorted(clips):
        f.write(f"        {ron_string(action)}: (\n")
        f.write(f"            animation: {ron_string(animation)},\n")
        f.write(f"            seconds: {ron_float(seconds)},\n")
        if events:
            inner = ", ".join(
                f"({ron_string(name)}, {ron_float(at)})" for name, at in events)
            f.write(f"            events: [{inner}],\n")
        f.write("        ),\n")
    f.write("    },\n")
    f.write(")\n")

print(f"BAKE bones={bones} height={height:.2f} clips={len(clips)} fps={fps:.3f}")
for action, animation, seconds, events in sorted(clips):
    print(f"BAKE clip {action} <- {animation} seconds={seconds:.3f} events={len(events)}")
