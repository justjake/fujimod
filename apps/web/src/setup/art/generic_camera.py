# Generic camera (not any real model) in Fujifilm design language, drawn as owners-manual line art.
# blender -b --python src/setup/art/generic_camera.py -- <rear34|front34|top|bottom> <out.png>
import bpy, bmesh, math, sys, mathutils
from mathutils import Vector
bpy.ops.wm.read_factory_settings(use_empty=True)
sc = bpy.context.scene
argv = sys.argv[sys.argv.index('--') + 1:] if '--' in sys.argv else []
view = argv[0] if argv else 'rear34'
out = argv[1] if len(argv) > 1 else f'/tmp/gc/{view}.png'

def mat(name, rgb):
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    m.use_nodes = True; nt = m.node_tree; nt.nodes.clear()
    o = nt.nodes.new('ShaderNodeOutputMaterial'); d = nt.nodes.new('ShaderNodeBsdfDiffuse'); s2r = nt.nodes.new('ShaderNodeShaderToRGB')
    ramp = nt.nodes.new('ShaderNodeValToRGB'); e = nt.nodes.new('ShaderNodeEmission')
    ramp.color_ramp.interpolation = 'EASE'
    el = ramp.color_ramp.elements; el[0].position = 0.05; el[1].position = 0.55
    r, g, b = rgb
    el[0].color = (r * .78, g * .78, b * .78, 1); el[1].color = (r, g, b, 1)
    nt.links.new(d.outputs[0], s2r.inputs[0]); nt.links.new(s2r.outputs[0], ramp.inputs[0])
    nt.links.new(ramp.outputs[0], e.inputs[0]); nt.links.new(e.outputs[0], o.inputs[0])
    return m
def srgb(v): return ((v / 255) ** 2.2)
def grey(v): x = srgb(v); return (x, x, x)
M = {'body': mat('body', grey(214)), 'plate': mat('plate', grey(236)), 'dark': mat('dark', grey(150)), 'mid': mat('mid', grey(190)),
     'light': mat('light', grey(246)), 'screen': mat('screen', grey(178)), 'glass': mat('glass', grey(120))}

def obj(name, mesh_fn, material, loc=(0, 0, 0), rot=(0, 0, 0), bevel=0, segments=4):
    mesh_fn()
    o = bpy.context.active_object; o.name = name
    o.location = loc; o.rotation_euler = rot
    o.data.materials.clear(); o.data.materials.append(M[material])
    if bevel:
        bm = o.modifiers.new('bevel', 'BEVEL'); bm.width = bevel; bm.segments = segments; bm.limit_method = 'ANGLE'
    bpy.ops.object.shade_smooth_by_angle(angle=math.radians(40))
    return o
def box(name, size, loc, material, bevel=0, segments=4):
    o = obj(name, lambda: bpy.ops.mesh.primitive_cube_add(size=1), material, loc, bevel=bevel, segments=segments)
    o.scale = size; bpy.ops.object.transform_apply(scale=True)
    return o
def cyl(name, r, depth, loc, material, axis='Z', verts=64, bevel=0):
    rot = {'Z': (0, 0, 0), 'Y': (math.pi / 2, 0, 0), 'X': (0, math.pi / 2, 0)}[axis]
    return obj(name, lambda: bpy.ops.mesh.primitive_cylinder_add(vertices=verts, radius=r, depth=depth), material, loc, rot, bevel=bevel, segments=2)

# knurling: mark every other vertical side edge of a cylinder for the inner line set
def knurl(o):
    me = o.data
    attr = me.attributes.get('freestyle_edge') or me.attributes.new('freestyle_edge', 'BOOLEAN', 'EDGE')
    n = 0
    for e in me.edges:
        a, b = me.vertices[e.vertices[0]].co, me.vertices[e.vertices[1]].co
        if abs(a.x - b.x) < 1e-5 and abs(a.y - b.y) < 1e-5:
            attr.data[e.index].value = (n % 2 == 0); n += 1

# A generic camera in Fujifilm's design language (rangefinder proportions, silver plates
# around a leatherette band, tall knurled dials, eyepiece at the back's top corner).
# x across (grip end -x), y depth (front -y, back +y), z up.
M['leather'] = mat('leather', grey(184))
M['silver'] = mat('silver', grey(232))
M['black'] = mat('black', grey(120))
W, D = 12.6, 3.2
ZB, ZL, ZT = 0.55, 4.9, 1.5
box('bottom-plate', (W, D, ZB), (0, 0, ZB / 2), 'silver', bevel=0.22)
box('leather', (W - 0.16, D - 0.12, ZL), (0, 0, ZB + ZL / 2), 'leather', bevel=0.3)
box('top-plate', (W, D, ZT), (0, 0, ZB + ZL + ZT / 2), 'silver', bevel=0.45)
top = ZB + ZL + ZT
lx, lz = 1.4, 3.05
cyl('lens-base', 2.3, 0.35, (lx, -D / 2 - 0.15, lz), 'silver', 'Y', bevel=0.05)
focus = cyl('focus-ring', 2.15, 0.75, (lx, -D / 2 - 0.68, lz), 'black', 'Y', verts=128)
cyl('lens-band', 2.1, 0.2, (lx, -D / 2 - 1.15, lz), 'silver', 'Y', bevel=0.03)
aperture = cyl('aperture-ring', 2.05, 0.55, (lx, -D / 2 - 1.5, lz), 'silver', 'Y', verts=128)
cyl('front-rim', 1.95, 0.3, (lx, -D / 2 - 1.92, lz), 'black', 'Y', bevel=0.05)
cyl('glass', 1.45, 0.06, (lx, -D / 2 - 2.05, lz), 'glass', 'Y')
cyl('glass-inner', 0.8, 0.06, (lx, -D / 2 - 2.08, lz), 'black', 'Y')
box('vf-window', (1.9, 0.12, 1.05), (4.6, -D / 2 - 0.04, ZB + ZL + 0.75), 'glass', bevel=0.12)
box('vf-frame', (2.15, 0.08, 1.3), (4.6, -D / 2 - 0.0, ZB + ZL + 0.75), 'black', bevel=0.14)
box('grip-ridge', (1.0, 0.35, 3.6), (-W / 2 + 1.2, -D / 2 - 0.1, ZB + 2.5), 'leather', bevel=0.15)
cyl('af-lamp', 0.16, 0.1, (-0.6, -D / 2 - 0.04, ZB + ZL + 0.85), 'black', 'Y')
speed = cyl('speed-dial', 1.05, 0.85, (-2.6, 0.25, top + 0.42), 'silver', verts=120)
cyl('speed-cap', 0.92, 0.04, (-2.6, 0.25, top + 0.86), 'silver')
expd = cyl('exp-dial', 0.85, 0.62, (-5.0, 0.5, top + 0.31), 'silver', verts=104)
cyl('collar', 0.58, 0.18, (-4.0, -0.95, top + 0.09), 'silver', bevel=0.03)
cyl('shutter', 0.33, 0.42, (-4.0, -0.95, top + 0.3), 'silver', bevel=0.06)
cyl('release-thread', 0.11, 0.04, (-4.0, -0.95, top + 0.52), 'black')
lever = box('lever', (0.6, 0.3, 0.12), (-4.0 + 0.78, -0.95 - 0.25, top + 0.1), 'silver', bevel=0.05)
cyl('fn-button', 0.22, 0.14, (-1.15, -0.95, top + 0.07), 'black', bevel=0.03)
box('hot-shoe', (1.9, 1.75, 0.22), (0.9, 0.05, top + 0.11), 'black', bevel=0.04)
box('shoe-cover', (1.5, 1.5, 0.08), (0.9, 0.05, top + 0.25), 'black', bevel=0.03)
for o in (speed, expd, focus, aperture): knurl(o)
for sx in (-1, 1):
    cyl(f'lug{sx}', 0.24, 0.32, (sx * (W / 2 + 0.06), 0.1, ZB + ZL - 0.5), 'silver', 'X', bevel=0.04)
by = D / 2 + 0.02
box('eyepiece', (1.7, 0.45, 1.2), (4.7, by + 0.18, ZB + ZL + 0.7), 'black', bevel=0.16)
box('eyepiece-glass', (1.15, 0.08, 0.7), (4.7, by + 0.42, ZB + ZL + 0.7), 'glass', bevel=0.05)
box('screen-frame', (6.2, 0.14, 4.0), (1.55, by + 0.04, ZB + 2.45), 'black', bevel=0.12)
box('screen', (5.8, 0.05, 3.6), (1.55, by + 0.13, ZB + 2.45), 'screen')
cyl('dpad-ring', 0.95, 0.12, (-3.9, by + 0.06, ZB + 2.3), 'black', 'Y', bevel=0.03)
cyl('dpad', 0.82, 0.14, (-3.9, by + 0.1, ZB + 2.3), 'silver', 'Y', bevel=0.03)
cyl('dpad-centre', 0.34, 0.2, (-3.9, by + 0.13, ZB + 2.3), 'silver', 'Y', bevel=0.04)
for i, (x, z) in enumerate([(-2.9, ZB + 4.05), (-4.9, ZB + 4.05), (-2.9, ZB + 0.55), (-4.9, ZB + 0.55)]):
    cyl(f'key{i}', 0.3, 0.16, (x, by + 0.08, z), 'black', 'Y', bevel=0.04)
cyl('led', 0.1, 0.08, (-2.15, by + 0.05, ZB + 4.5), 'black', 'Y')
box('port-door', (0.08, 0.9, 1.5), (-W / 2 - 0.03, 0.2, ZB + 2.2), 'silver', bevel=0.03)

# ---- lines: silhouettes thick, inner edges thin ----
sc.render.use_freestyle = True
vl = sc.view_layers[0]; fs = vl.freestyle_settings
fs.crease_angle = math.radians(150)
ls = fs.linesets[0]; ls.name = 'outline'
ls.select_by_visibility = True; ls.select_by_edge_types = True
for k in ('select_silhouette', 'select_border', 'select_crease', 'select_material_boundary', 'select_external_contour', 'select_contour'):
    setattr(ls, k, k in ('select_silhouette', 'select_border', 'select_external_contour'))
style = ls.linestyle or bpy.data.linestyles.new('outline'); ls.linestyle = style
style.color = (0, 0, 0); style.thickness = 4.6; style.caps = 'ROUND'
inner = fs.linesets.new('inner')
inner.select_by_visibility = True; inner.select_by_edge_types = True
for k in ('select_silhouette', 'select_border', 'select_crease', 'select_material_boundary', 'select_contour', 'select_external_contour'):
    setattr(inner, k, k in ('select_crease', 'select_material_boundary', 'select_contour'))
inner.select_edge_mark = True
st2 = bpy.data.linestyles.new('inner'); inner.linestyle = st2; st2.color = (0, 0, 0); st2.thickness = 1.7; st2.caps = 'ROUND'

# light from the upper left
sun = bpy.data.objects.new('sun', bpy.data.lights.new('sun', 'SUN')); sc.collection.objects.link(sun)
sun.rotation_euler = (math.radians(35), math.radians(-25), math.radians(-30)); sun.data.energy = 3

# camera
dirs = {'rear34': Vector((-0.55, 1.0, 0.62)), 'front34': Vector((0.5, -1.0, 0.45)), 'top': Vector((0.0, 0.25, 1.0)), 'bottom': Vector((0.4, 0.8, -0.7))}
d = dirs[view].normalized()
cam = bpy.data.objects.new('cam', bpy.data.cameras.new('cam')); sc.collection.objects.link(cam)
cam.data.type = 'ORTHO'; cam.data.ortho_scale = 18
target = Vector((0.2, -0.6, 3.6))
cam.location = target + d * 40
cam.rotation_euler = (-d).to_track_quat('-Z', 'Y').to_euler()
sc.camera = cam
sc.render.engine = 'BLENDER_EEVEE'
sc.render.resolution_x, sc.render.resolution_y = 1600, 1100
sc.render.film_transparent = True
sc.view_settings.view_transform = 'Standard'
sc.render.filepath = out
bpy.ops.render.render(write_still=True)
