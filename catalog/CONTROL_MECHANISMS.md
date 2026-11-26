# Relative dial and rocker review

Reviewed every body control document against its owner's manual and the
connections in its `body.json`. Page numbers below are printed manual page
numbers. The links identify the editions used by `controls.json`.

The X70 (page 6) and X100S (page 5) command controls press left and right and
have three independent active-low GPIO contacts. They are rockers. The X100
command control (page 5) has the same documented motion; its connections are
unlocated. It is also a rocker, correcting its former four-direction pad
classification. X100 and X100S also have a separate rotating command dial.

All other relative dials below rotate. Command-dial sections distinguish
rotation from a center press; function dials rotate to change their assigned
setting. Mechanical diopter controls rotate but have no electronic actions.
A push contact alone does not establish the rotation wiring: X100T, X30,
X-E2, and X-T10 currently locate only that contact. An encoder connection
contains the two rotation phases; a separate GPIO connection beside it is
the center press. No located connection means the manual establishes the
mechanism while its electrical implementation remains unlocated.

The X70 rocker drawing follows page 6: a bowed knurled band, smooth center
tab, curved direction arrow, and solid arrow into the tab for the center press.

The review covers 49 bodies and 108 relative dials/rockers (including mechanical diopters).

| Body | Control | Manual page | Decision | Catalog connections checked |
| --- | --- | --- | --- | --- |
| x10 | `main_command_dial` | [4](https://dl.fujifilm-x.com/support/manual/x/fujifilm_x10_manual_en.pdf) | Dial: rotate | No located connection |
| x10 | `sub_command_dial` | [4](https://dl.fujifilm-x.com/support/manual/x/fujifilm_x10_manual_en.pdf) | Dial: rotate | No located connection |
| x100 | `command_dial` | [4](https://dl.fujifilm-x.com/support/manual/x/finepix_x100_manual_en.pdf) | Dial: rotate | No located connection |
| x100 | `command_control` | [5](https://dl.fujifilm-x.com/support/manual/x/finepix_x100_manual_en.pdf) | Rocker: press left/right | No located connection |
| x100f | `front_command_dial` | [6](https://fujifilm-dsc.com/en/manual/x100f_v20/x100f_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| x100f | `diopter_adjustment_control` | [29](https://fujifilm-dsc.com/en/manual/x100f_v20/x100f_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| x100f | `rear_command_dial` | [6](https://fujifilm-dsc.com/en/manual/x100f_v20/x100f_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| x100s | `command_control` | [5](https://dl.fujifilm-x.com/support/manual/x/fujifilm_x100s_manual_en.pdf) | Rocker: press left/right | `controls-gpio-3-8`, `controls-gpio-3-9`, `controls-gpio-3-10` |
| x100s | `command_dial` | [4](https://dl.fujifilm-x.com/support/manual/x/fujifilm_x100s_manual_en.pdf) | Dial: rotate | `controls-encoder-3-11-3-12` |
| x100t | `command_dial` | [16](https://fujifilm-dsc.com/en/manual/x100t/x100t_om-full_en_01.pdf) | Dial: rotate | `controls-gpio-0-10` |
| x100v | `front_command_dial` | [224](https://fujifilm-dsc.com/en/manual/x100v/x100v_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| x100v | `diopter_adjustment_control` | [8](https://fujifilm-dsc.com/en/manual/x100v/x100v_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| x100v | `rear_command_dial` | [10](https://fujifilm-dsc.com/en/manual/x100v/x100v_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| x100vi | `front_command_dial` | [10](https://app.fujifilm-dsc.com/en/manual/x100vi/x100vi_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| x100vi | `rear_command_dial` | [10](https://app.fujifilm-dsc.com/en/manual/x100vi/x100vi_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| x30 | `command_dial` | [16](https://fujifilm-dsc.com/en/manual/x30/x30_om-full_en_01.pdf) | Dial: rotate | `controls-gpio-0-10` |
| x70 | `command_control` | [6](https://fujifilm-dsc.com/en/manual/x70/X70_OMW_EN_S_F_02.pdf) | Rocker: press left/right | `controls-gpio-0-10`, `controls-gpio-3-8`, `controls-gpio-3-9` |
| xa1 | `main_command_dial` | [5](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xa1_manual_en.pdf) | Dial: rotate | No located connection |
| xa1 | `sub_command_dial` | [5](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xa1_manual_en.pdf) | Dial: rotate | `controls-encoder-3-8-3-9`, `controls-gpio-0-10` |
| xa10 | `main_command_dial` | [4](https://fujifilm-dsc.com/en/manual/x-a10/x-a10_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xa10 | `sub_command_dial` | [4](https://fujifilm-dsc.com/en/manual/x-a10/x-a10_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xa2 | `main_command_dial` | [5](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xa2_manual_en.pdf) | Dial: rotate | No located connection |
| xa2 | `sub_command_dial` | [5](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xa2_manual_en.pdf) | Dial: rotate | `controls-encoder-3-8-3-9`, `controls-gpio-0-10` |
| xa3 | `main_command_dial` | [4](https://fujifilm-dsc.com/en/manual/x-a3/x-a3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xa3 | `sub_command_dial` | [4](https://fujifilm-dsc.com/en/manual/x-a3/x-a3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xa5 | `main_command_dial` | [6](https://fujifilm-dsc.com/en-int/manual/x-a5/x-a5_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xa5 | `sub_command_dial` | [6](https://fujifilm-dsc.com/en-int/manual/x-a5/x-a5_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xa7 | `rear_command_dial` | [6](https://fujifilm-dsc.com/en/manual/x-a7/x-a7_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xa7 | `front_command_dial` | [6](https://fujifilm-dsc.com/en/manual/x-a7/x-a7_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xe1 | `command_dial` | [4](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xe1_manual_en.pdf) | Dial: rotate | No located connection |
| xe1 | `diopter_adjustment_control` | [18](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xe1_manual_en.pdf) | Dial: rotate | No located connection |
| xe2 | `command_dial` | [5](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xe2_manual_en.pdf) | Dial: rotate | `controls-gpio-0-10` |
| xe2s | `command_dial` | [5](https://fujifilm-dsc.com/en-int/manual/x-e2s/x-e2s_omw_en_s_f.pdf) | Dial: rotate | `controls-gpio-0-10`, `controls-encoder-3-8-3-9` |
| xe3 | `front_command_dial` | [8](https://fujifilm-dsc.com/en/manual/x-e3/x-e3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xe3 | `diopter_adjustment_control` | [11](https://fujifilm-dsc.com/en/manual/x-e3/x-e3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xe3 | `rear_command_dial` | [253](https://fujifilm-dsc.com/en/manual/x-e3/x-e3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xe4 | `front_command_dial` | [2](https://app.fujifilm-dsc.com/en/manual/x-e4/x-e4_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xe4 | `diopter_adjustment_control` | [3](https://app.fujifilm-dsc.com/en/manual/x-e4/x-e4_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xe5 | `diopter_adjustment_control` | [15](https://fujifilm-dsc.com/en/manual/x-e5/x-e5_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xe5 | `front_command_dial` | [312](https://fujifilm-dsc.com/en/manual/x-e5/x-e5_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xe5 | `rear_command_dial` | [339](https://fujifilm-dsc.com/en/manual/x-e5/x-e5_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xf1 | `main_command_dial` | [4](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xf1_manual_en.pdf) | Dial: rotate | No located connection |
| xf1 | `sub_command_dial` | [4](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xf1_manual_en.pdf) | Dial: rotate | No located connection |
| xh1 | `diopter_adjustment_control` | [8](https://app.fujifilm-dsc.com/en/manual/x-h1/x-h1_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xh1 | `front_command_dial` | [196](https://app.fujifilm-dsc.com/en/manual/x-h1/x-h1_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xh1 | `rear_command_dial` | [217](https://app.fujifilm-dsc.com/en/manual/x-h1/x-h1_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xh2 | `front_command_dial` | [10](https://fujifilm-dsc.com/en-int/manual/x-h2/x-h2_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xh2 | `rear_command_dial` | [10](https://fujifilm-dsc.com/en-int/manual/x-h2/x-h2_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xh2 | `diopter_adjustment_control` | [13](https://fujifilm-dsc.com/en-int/manual/x-h2/x-h2_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xh2s | `front_command_dial` | [10](https://app.fujifilm-dsc.com/en-int/manual/x-h2s/x-h2s_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xh2s | `rear_command_dial` | [10](https://app.fujifilm-dsc.com/en-int/manual/x-h2s/x-h2s_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xh2s | `diopter_adjustment_control` | [13](https://app.fujifilm-dsc.com/en-int/manual/x-h2s/x-h2s_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xm1 | `main_command_dial` | [5](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xm1_manual_en.pdf) | Dial: rotate | No located connection |
| xm1 | `sub_command_dial` | [5](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xm1_manual_en.pdf) | Dial: rotate | `controls-encoder-3-8-3-9`, `controls-gpio-0-10` |
| xm5 | `rear_command_dial` | [9](https://app.fujifilm-dsc.com/en/manual/x-m5/x-m5_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xm5 | `front_command_dial` | [10](https://app.fujifilm-dsc.com/en/manual/x-m5/x-m5_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xpro1 | `command_dial` | [4](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xpro1_manual_en.pdf) | Dial: rotate | No located connection |
| xpro2 | `diopter_adjustment_control` | [31](https://fujifilm-dsc.com/en-int/manual/x-pro2/x-pro2_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xpro2 | `front_command_dial` | [5](https://fujifilm-dsc.com/en-int/manual/x-pro2/x-pro2_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xpro2 | `rear_command_dial` | [6](https://fujifilm-dsc.com/en-int/manual/x-pro2/x-pro2_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xpro3 | `front_command_dial` | [9](https://app.fujifilm-dsc.com/en/manual/x-pro3/x-pro3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xpro3 | `diopter_adjustment_control` | [8](https://app.fujifilm-dsc.com/en/manual/x-pro3/x-pro3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xpro3 | `rear_command_dial` | [9](https://app.fujifilm-dsc.com/en/manual/x-pro3/x-pro3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xs1 | `command_dial` | [23](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xs1_manual_en.pdf) | Dial: rotate | No located connection |
| xs10 | `rear_command_dial` | [2](https://app.fujifilm-dsc.com/en/manual/x-s10/x-s10_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xs10 | `function_dial` | [2](https://app.fujifilm-dsc.com/en/manual/x-s10/x-s10_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xs10 | `front_command_dial` | [2](https://app.fujifilm-dsc.com/en/manual/x-s10/x-s10_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xs10 | `diopter_adjustment_control` | [3](https://app.fujifilm-dsc.com/en/manual/x-s10/x-s10_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xs20 | `rear_command_dial` | [6](https://app.fujifilm-dsc.com/en/manual/x-s20/x-s20_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xs20 | `fn_dial` | [7](https://app.fujifilm-dsc.com/en/manual/x-s20/x-s20_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xs20 | `front_command_dial` | [6](https://app.fujifilm-dsc.com/en/manual/x-s20/x-s20_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xs20 | `diopter_adjustment_control` | [9](https://app.fujifilm-dsc.com/en/manual/x-s20/x-s20_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt1 | `rear_command_dial` | [8](https://dl.fujifilm-x.com/support/manual/x/fujifilm_xt1_manual_en.pdf) | Dial: rotate | No located connection |
| xt10 | `rear_command_dial` | [6](https://fujifilm-dsc.com/en/manual/x-t10/x-t10_omw_en_s_f_02.pdf) | Dial: rotate | `controls-gpio-0-10` |
| xt100 | `function_dial` | [7](https://fujifilm-dsc.com/en/manual/x-t100/x-t100_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt100 | `main_command_dial` | [6](https://fujifilm-dsc.com/en/manual/x-t100/x-t100_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt100 | `diopter_adjustment_control` | [9](https://fujifilm-dsc.com/en/manual/x-t100/x-t100_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt100 | `sub_command_dial` | [6](https://fujifilm-dsc.com/en/manual/x-t100/x-t100_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt2 | `diopter_adjustment_control` | [11](https://fujifilm-dsc.com/en/manual/x-t2/x-t2_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt2 | `front_command_dial` | [7](https://fujifilm-dsc.com/en/manual/x-t2/x-t2_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt2 | `rear_command_dial` | [7](https://fujifilm-dsc.com/en/manual/x-t2/x-t2_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt20 | `front_command_dial` | [8](https://app.fujifilm-dsc.com/en/manual/x-t20_v20/x-t20_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt20 | `diopter_adjustment_control` | [11](https://app.fujifilm-dsc.com/en/manual/x-t20_v20/x-t20_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt20 | `rear_command_dial` | [9](https://app.fujifilm-dsc.com/en/manual/x-t20_v20/x-t20_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt200 | `rear_command_dial` | [6](https://app.fujifilm-dsc.com/en/manual/x-t200/x-t200_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt200 | `function_dial` | [7](https://app.fujifilm-dsc.com/en/manual/x-t200/x-t200_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt200 | `front_command_dial` | [6](https://app.fujifilm-dsc.com/en/manual/x-t200/x-t200_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt200 | `diopter_adjustment_control` | [9](https://app.fujifilm-dsc.com/en/manual/x-t200/x-t200_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt3 | `front_command_dial` | [7](https://fujifilm-dsc.com/en/manual/x-t3/x-t3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt3 | `rear_command_dial` | [73](https://fujifilm-dsc.com/en/manual/x-t3/x-t3_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt30 | `front_command_dial` | [207](https://app.fujifilm-dsc.com/en/manual/x-t30/x-t30_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt30 | `diopter_adjustment_control` | [9](https://app.fujifilm-dsc.com/en/manual/x-t30/x-t30_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt30 | `rear_command_dial` | [207](https://app.fujifilm-dsc.com/en/manual/x-t30/x-t30_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt30ii | `front_command_dial` | [7](https://fujifilm-dsc.com/en-int/manual/x-t30-2/x-t30-2_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt30ii | `diopter_adjustment_control` | [9](https://fujifilm-dsc.com/en-int/manual/x-t30-2/x-t30-2_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt30ii | `rear_command_dial` | [7](https://fujifilm-dsc.com/en-int/manual/x-t30-2/x-t30-2_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt30iii | `front_command_dial` | [9](https://fujifilm-dsc.com/en/manual/x-t30-iii/x-t30-iii_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt30iii | `diopter_adjustment_control` | [13](https://fujifilm-dsc.com/en/manual/x-t30-iii/x-t30-iii_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt30iii | `rear_command_dial` | [9](https://fujifilm-dsc.com/en/manual/x-t30-iii/x-t30-iii_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt4 | `diopter_adjustment_control` | [9](https://fujifilm-dsc.com/en/manual/x-t4/x-t4_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt4 | `front_command_dial` | [8](https://fujifilm-dsc.com/en/manual/x-t4/x-t4_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt4 | `rear_command_dial` | [255](https://fujifilm-dsc.com/en/manual/x-t4/x-t4_omw_en_s_f.pdf) | Dial: rotate | No located connection |
| xt5 | `diopter_adjustment_control` | [11](https://app.fujifilm-dsc.com/en-int/manual/x-t5/x-t5_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt5 | `front_command_dial` | [7](https://app.fujifilm-dsc.com/en-int/manual/x-t5/x-t5_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt5 | `rear_command_dial` | [321](https://app.fujifilm-dsc.com/en-int/manual/x-t5/x-t5_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt50 | `front_command_dial` | [9](https://app.fujifilm-dsc.com/en/manual/x-t50/x-t50_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt50 | `diopter_adjustment_control` | [13](https://app.fujifilm-dsc.com/en/manual/x-t50/x-t50_manual_en_s_f.pdf) | Dial: rotate | No located connection |
| xt50 | `rear_command_dial` | [9](https://app.fujifilm-dsc.com/en/manual/x-t50/x-t50_manual_en_s_f.pdf) | Dial: rotate | No located connection |

Bodies without a relative dial or rocker: `x20`, `xhalf`, `xq`. Their positional dials, rings, levers, and pads retain their declared mechanisms.
