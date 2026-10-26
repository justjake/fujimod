# Renders FujiMod's Fujifilm X70 model as owner's-manual line art, in layers for the
# setup wizard's Lottie scenes:
#   blender -b --python src/setup/art/x70_layers.py -- <view> <out-dir> [model.glb] [freestyle-svg-addon.py]
# Writes, for one view: fill.png (flat greys, 2x), lines.svg (vector line art, 1x),
# lever-<k>-fill.png / lever-<k>.svg (the power lever alone at LEVER_STEPS angles from ON to OFF)
# and anchors.json (screen positions, in 1x pixels, of the parts the overlays point at).
# The SVG exporter is Blender's former render_freestyle_svg add-on (GPL), loaded from a path.
import bpy, math, sys, os, json, importlib.util
from mathutils import Vector, Matrix
from bpy_extras.object_utils import world_to_camera_view

argv = sys.argv[sys.argv.index('--') + 1:]
view, outdir = argv[0], argv[1]
here = os.path.dirname(os.path.abspath(__file__))
model = argv[2] if len(argv) > 2 else os.path.join(here, '../../../../../crates/simulation/assets/x70.glb')
addon = argv[3] if len(argv) > 3 else os.path.join(here, '../../../../../../.tools/blender-addons/render_freestyle_svg.py')
W, H = 1600, 1100
LEVER_STEPS = 7
LEVER_OFF_DEG = -34          # lever travel about the shutter axis, ON (0) to OFF

bpy.ops.wm.read_factory_settings(use_empty=True)
sc = bpy.context.scene
spec = importlib.util.spec_from_file_location('render_freestyle_svg', addon)
svgmod = importlib.util.module_from_spec(spec); spec.loader.exec_module(svgmod); svgmod.register()

def flat(name, v):
    m = bpy.data.materials.new(name); m.use_nodes = True; nt = m.node_tree; nt.nodes.clear()
    o = nt.nodes.new('ShaderNodeOutputMaterial'); e = nt.nodes.new('ShaderNodeEmission')
    x = (v / 255) ** 2.2; e.inputs[0].default_value = (x, x, x, 1)
    nt.links.new(e.outputs[0], o.inputs[0]); return m
TONES = {'m_silver_mat': 238, 'm_black_mat': 186, 'm_black_gros': 160, 'm_leather': 194, 'm_screen': 150, 'm_flash': 126,
         'M_LENS_UNIT': 122, 'm_red': 150, 'm_white': 250, 'm_yellow': 220, 'material_0': 200}
mats = {k: flat('f_' + k, v) for k, v in TONES.items()}
bpy.ops.import_scene.gltf(filepath=model)
HIDDEN = ('ling_filter_21', 'ling_filter_glass_22', 'LBL_filter_14', 'hide_wall_56')
def node(o):
    p = o
    while p.parent and p.parent.parent: p = p.parent
    return p.name if o.parent else o.name
parts = {}
for o in bpy.data.objects:
    if o.type != 'MESH': continue
    n = o.parent.name if o.parent else o.name
    parts.setdefault(n, []).append(o)
    if n in HIDDEN: o.hide_render = True
    for slot in o.material_slots:
        mn = slot.material.name if slot.material else ''
        if mn == 'm_logo': o.hide_render = True
        else: slot.material = mats.get(mn, mats['m_black_mat'])
meshes = [o for o in bpy.data.objects if o.type == 'MESH' and not o.hide_render]
lever = parts['on-off_38']
shutter = parts['shutter_28']

def bounds(objs):
    pts = [o.matrix_world @ Vector(c) for o in objs for c in o.bound_box]
    return Vector([min(p[i] for p in pts) for i in range(3)]), Vector([max(p[i] for p in pts) for i in range(3)])
lo, hi = bounds(meshes)
ctr, size = (lo + hi) / 2, (hi - lo).length

# lines: heavy silhouettes, thin material boundaries and real creases
sc.render.use_freestyle = True
fs = sc.view_layers[0].freestyle_settings
fs.crease_angle = math.radians(110)
def lineset(ls, kinds, name, width):
    ls.name = name; ls.select_by_visibility = True; ls.select_by_edge_types = True
    for k in ('select_silhouette', 'select_border', 'select_crease', 'select_material_boundary', 'select_contour', 'select_external_contour'):
        setattr(ls, k, k in kinds)
    st = bpy.data.linestyles.new(name); ls.linestyle = st; st.color = (0, 0, 0); st.thickness = width; st.caps = 'ROUND'
lineset(fs.linesets[0], ('select_silhouette', 'select_border', 'select_external_contour'), 'outline', 4.2)
inner = fs.linesets.new('inner')
lineset(inner, ('select_crease', 'select_material_boundary', 'select_contour'), 'inner', 1.6)
# densely knurled small parts draw as scribbles: keep only their silhouettes
NOISY = ('zoom_sw_36', 'drive_37', 'MCS_31', 'MCS2_52', 'MCS3_53', 'sw_auto_41', 'strap attachment_34', 'shutter_speed_pos_58', 'Exposure_pos_57')
quiet = bpy.data.collections.new('quiet-lines'); sc.collection.children.link(quiet)
for n in NOISY:
    for o in parts.get(n, []): quiet.objects.link(o)
inner.select_by_collection = True; inner.collection = quiet; inner.collection_negation = 'EXCLUSIVE'

dirs = {'rear34': (-0.55, 1.0, 0.62), 'front34': (0.5, -1.0, 0.45), 'top': (0, 0.18, 1), 'back': (0, 1, 0.08), 'bottom': (-0.45, 0.75, -0.9)}
d = Vector(dirs[view]).normalized()
cam = bpy.data.objects.new('cam', bpy.data.cameras.new('cam')); sc.collection.objects.link(cam)
cam.data.type = 'ORTHO'; cam.data.ortho_scale = size * 1.02
cam.location = ctr + d * size * 4; cam.rotation_euler = (-d).to_track_quat('-Z', 'Y').to_euler(); sc.camera = cam
sc.render.engine = 'BLENDER_EEVEE'
sc.render.resolution_x, sc.render.resolution_y = W, H
sc.render.film_transparent = True; sc.view_settings.view_transform = 'Standard'
sc.frame_start = sc.frame_end = sc.frame_current = 1
sv = sc.svg_export; sv.mode = 'FRAME'; sv.object_fill = False; sv.split_at_invisible = False
os.makedirs(outdir, exist_ok=True)

def show(objs):
    keep = set(map(id, objs))
    for o in meshes: o.hide_render = id(o) not in keep
def render(name, lines):
    sc.render.use_freestyle = lines; sv.use_svg_export = lines
    sc.render.resolution_percentage = 100 if lines else 200
    sc.render.filepath = os.path.join(outdir, name)
    bpy.ops.render.render(write_still=True)
    if lines:  # the add-on appends the frame number
        os.replace(os.path.join(outdir, name + '0001.svg'), os.path.join(outdir, name + '.svg'))
        os.remove(os.path.join(outdir, name + '.png'))

def px(p):
    v = world_to_camera_view(sc, cam, p); return [round(v.x * W, 1), round((1 - v.y) * H, 1)]
body = [o for o in meshes if o not in lever]
show(body); render('fill', False); render('lines', True)

# the lever at each step from ON to OFF, rotated about the shutter's axis
s_lo, s_hi = bounds(shutter); pivot = (s_lo + s_hi) / 2
axis = Vector((0, 0, 1))
base = {o.name: o.matrix_world.copy() for o in lever}
lever_path = []
for k in range(LEVER_STEPS):
    a = math.radians(LEVER_OFF_DEG * k / (LEVER_STEPS - 1))
    R = Matrix.Translation(pivot) @ Matrix.Rotation(a, 4, axis) @ Matrix.Translation(-pivot)
    for o in lever: o.matrix_world = R @ base[o.name]
    show(lever); render(f'lever-{k}-fill', False); render(f'lever-{k}', True)
    la, lb = bounds(lever); lever_path.append(px((la + lb) / 2))
for o in lever: o.matrix_world = base[o.name]

def centre(name):
    a, b = bounds(parts[name]); return px((a + b) / 2)
def top_centre(name):
    a, b = bounds(parts[name]); return px(Vector(((a.x + b.x) / 2, (a.y + b.y) / 2, b.z)))
sa, sb = bounds(parts['monitorr_screen_46'])
corners = [px(Vector((x, y, z))) for x, y, z in ((sa.x, sb.y, sb.z), (sb.x, sb.y, sb.z), (sb.x, sb.y, sa.z), (sa.x, sb.y, sa.z))]
ma, mb = bounds(parts['menu_sw_49'])
dpad_c = (ma + mb) / 2
anchors = {
    'view': view, 'size': [W, H],
    'shutter': top_centre('shutter_28'), 'pivot': px(pivot),
    'dpad': px(dpad_c), 'up': px(Vector((dpad_c.x, mb.y, dpad_c.z + (mb.z - ma.z) * 0.38))),
    'port': centre('HDMI_door_55'), 'screen': corners,
    'bottom': centre('body.bottom_set_50'), 'lever': lever_path,
}
with open(os.path.join(outdir, 'anchors.json'), 'w') as f: json.dump(anchors, f, indent=1)
print('ANCHORS', json.dumps(anchors))
