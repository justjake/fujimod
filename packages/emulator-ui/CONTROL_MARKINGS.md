# Control markings and mini controls

A camera control has one presentation everywhere it appears: on the emulator's
control panel, in the panel's power-on combos, on the Recovery page and in the
Dev Tools service-mode guide. That presentation comes from catalog facts about
the control and is drawn by the panel's own components. Nothing derives a
control's appearance by matching its manual name.

## Catalog facts

`crates/catalog-controls` defines `PhysicalControl`; each body's facts live in
`catalog/bodies/<body>/controls.json` and are hand-authored.

- `face: Option<String>`: text printed on the control, exactly as printed. A
  `/` separates stacked lines (`DISP/BACK`, `MENU/OK`, `AF-L/AE-L`).
- `icon: Option<ControlIcon>`: the printed symbol. `ControlIcon` is a closed
  enum, serialized in snake case: `playback`, `delete`, `record` (the red
  movie-record dot), `flash`. A new symbol is a new enum variant.
- `short_name: Option<String>`: the name used beside a control that carries no
  printed marking, at most 10 uppercase characters, spelled out where it fits
  (`COMMAND`, `FRONT DIAL`, `REAR DIAL`, `MAIN DIAL`, `SUB DIAL`, `EV`, `SPEED`,
  `APERTURE`, `FOCUS`, `MODE`, `DRIVE`, `ZOOM`).

`ControlBehavior::Rocker { decrease, increase, press }` is a control tipped
left or right that springs back to center, with an optional center press: the
X70's command control. It is not a dial; a dial turns continuously through
detents. A rocker wires one contact per direction, where a dial's rotation runs
through an encoder. Every control modeled as a stepping `dial` is checked
against its manual (rotate versus press left or right) and its wiring, and
recorded as one or the other. See the [completed mechanism review](../../catalog/CONTROL_MECHANISMS.md).

The catalog check fails when a non-mechanical, non-touch control has none of
`face`, `icon` or `short_name`, and when any field breaks the limits above.

`boot` records each mode a body supports. Every body with `boot.sd` also
records `boot.firmware_update`, from its manual's firmware update procedure.

Existing bodies are brought up to this rule with a one-time migration that
writes the facts into `controls.json` for review. The free-text icons it finds
(`playback`, `delete`, `movie recording`, `flash pop-up`) become enum values.
The migration is not kept.

## One mapping from control to component

`src/ui/control-part.ts` maps a catalog control, and optionally one of its
actions, to the panel component that draws it. The mapping reads
`behavior.kind` and the action; it never reads `name`.

```ts
export interface Marking {
  face?: string[];
  icon?: ControlIcon;
  shortName?: string;
}
export type ControlPart =
  | { kind: 'key'; marking: Marking }
  | { kind: 'shutter'; marking: Marking; stage?: 'half' | 'full' }
  | { kind: 'command'; marking: Marking; held?: 'press' | 'decrease' | 'increase' }
  | { kind: 'rocker'; marking: Marking; held?: 'press' | 'decrease' | 'increase' }
  | { kind: 'pad'; marking: Marking; held?: 'up' | 'down' | 'left' | 'right' | 'press' }
  | { kind: 'positions'; marking: Marking; position?: string };

export function marking(control: PhysicalControl): Marking;
export function controlPart(control: PhysicalControl, action?: string, stage?: string): ControlPart;
```

| `behavior.kind` | Part | Held state from a boot hold |
| --- | --- | --- |
| `button` | `key` | none |
| `shutter` | `shutter` | `stage` |
| `dial`, `ring` with steps | `command` | the action after `.` (`press`, `decrease`, `increase`) |
| `rocker` | `rocker` | the action after `.` (`press`, `decrease`, `increase`) |
| `pad` | `pad` | the direction or `press` |
| `switch`, `lever`, positional `dial` or `ring` | `positions` | the position |

`controlPart` throws for an unknown control id or an action the control does
not declare. A control's `pushable` fact admits `.press` when its binding is
not located.

## Shared drawing

The live components in `src/ui/controls.tsx` split into a drawing part and an
interactive wrapper. The drawing parts move to `src/ui/control-faces.tsx`:
`KeyFace`, `ShutterFace`, `CommandFace`, `RockerFace`, `PadFace` and
`PositionsFace`. Each
takes a `ControlPart` and a `size: 'panel' | 'mini'`, and draws the held part
with the panel's `glow` highlight, the one that marks a combo's controls when
the combo is hovered. A key glows as a whole; a command control glows only at
its held side, a D-pad only at the held direction. A mini command control
whose press is held is drawn as its own icon instead: the knurled wheel seen
edge-on with an arrow pushing into it, without the turn arrows.

`RockerFace` follows the owner's manual drawing of a rocker: a knurled band
with a smooth tab at its center. On the panel, its left and right halves tip it
and the tab presses it. A mini with its press held adds a solid arrow pushing
into the tab; with a direction held, a curved arrow pointing to that side. The panel's `Key`, `Shutter`, `Dial` and
D-pad/command nodes wrap these with `ControlSession` handlers, so a change to
how a control looks reaches every place it appears.

`mini` scales the same markup through one CSS custom property. A mini shows
its `short_name` beside it only when the marking has neither `face` nor `icon`.

## Power-on combos

`src/ui/power-combo.tsx` exports `PowerComboKeys`:

```ts
export function PowerComboKeys(props: {
  controls: PhysicalControls;
  modes: BootMode[];
}): JSX.Element;
export function bootSequence(controls: PhysicalControls, modes: BootMode[]): {
  usb?: boolean;
  hold: BootHold[];
};
```

`bootSequence` joins the holds of the given modes in order, without duplicates,
and throws when the body lacks one of the modes or two modes disagree about
USB. `PowerComboKeys` draws, in order: a USB plug icon (connected or
disconnected) when the sequence states USB, one mini per hold, then the power
switch as a mini `PositionsFace` at `ON`. Minis wrap to further lines rather
than overlapping.

Users:

- The panel's POWER-ON COMBOS: `Combo` wraps `PowerComboKeys` with its hover,
  highlight and run behavior. `PowerCombo` keeps `label`, `modes` and the
  derived `inputs`; its `keys` field is removed.
- Recovery's boot step draws `PowerComboKeys` with modes `sd` and
  `firmware_update`, replacing the list of sentences and the fixed DISP/BACK
  line.
- The Dev Tools service-mode guide draws `PowerComboKeys` with mode `service`
  in place of the hold sentence.

## Panels

`defaultLayout` takes the body's `PhysicalControls` beside the session's
`ControlInput` list. Inputs still decide what is supported and which positions
exist; the face, icon and caption of each node come from `marking`. Grouping
into D-pads and command controls reads `behavior.kind` instead of id suffixes.
Device panels such as `devices/x70/panel.json` keep their explicit nodes.

The session controller receives the body's `PhysicalControls` with the boot
configuration from the web app, which reads them from the catalog.

## Removed

`controlFace`, its glyph table and the caption table in `default-layout.ts`;
`describeBootHold` and the key guessing in `panelBootCombos` in
`apps/web/src/cameras/boot-options.ts`; `PowerCombo.keys`. The "Default
layouts" section of `apps/web/EMULATOR_UI.md` is rewritten to point here.

## Checks

- The catalog check covers marking completeness, `ControlIcon` values,
  `short_name` limits and `firmware_update` beside `sd` for every body.
- Unit tests map every boot hold of every body through `controlPart` without
  error and cover each row of the mapping table.
- Component tests draw the X70's `sd` and `firmware_update` sequence: the
  rocker with the push arrow into its tab and `COMMAND` beside it, the record
  key,
  the DISP/BACK key, and the power switch at ON.
- A development-only page at `/review/power-combos/` draws every body's SD
  sequence at phone width for visual review, titled with each body's model
  name.
