//! Bounded execution of the uploaded vector instruction memory.
//! Field layout follows the SMAPH arithmetic/flow encoding (also documented
//! by nihstro's shader_bytecode.h); no program identity selects behavior.
use super::Engine;

pub type Vector = [f32; 4];

fn condition(word: u32, flags: [bool; 2]) -> bool {
    let x = flags[0] == (word & (1 << 25) != 0);
    let y = flags[1] == (word & (1 << 24) != 0);
    match word >> 22 & 3 {
        0 => x || y,
        1 => x && y,
        2 => x,
        _ => y,
    }
}
fn compare(a: f32, b: f32, op: u32) -> Result<bool, String> {
    Ok(match op {
        0 => a == b,
        1 => a != b,
        2 => a < b,
        3 => a <= b,
        4 => a > b,
        5 => a >= b,
        _ => return Err("DMP unsupported shader comparison".into()),
    })
}
#[derive(Clone, Copy)]
enum Frame {
    Call {
        end: usize,
        back: usize,
    },
    If {
        end: usize,
        back: usize,
    },
    Loop {
        end: usize,
        back: usize,
        remaining: u32,
        increment: i32,
        previous: i32,
    },
}
struct Machine<'a> {
    engine: &'a Engine,
    inputs: &'a [Vector; 16],
    temporary: [Vector; 16],
    output: [Vector; 16],
    address: [i32; 3],
}
impl Machine<'_> {
    fn source(
        &self,
        index: u32,
        relative: u32,
        selector: u32,
        negate: bool,
    ) -> Result<Vector, String> {
        let value = if index < 16 {
            self.inputs[index as usize]
        } else if index < 32 {
            self.temporary[index as usize - 16]
        } else {
            let offset = if relative == 0 {
                0
            } else {
                self.address[relative as usize - 1]
            };
            let slot = index as i32 - 32 + offset;
            self.engine
                .uniforms
                .get(slot as usize)
                .ok_or_else(|| format!("DMP shader uniform {slot} outside bank"))?
                .map(f32::from_bits)
        };
        Ok(std::array::from_fn(|i| {
            let v = value[(selector >> (6 - 2 * i)) as usize & 3];
            if negate { -v } else { v }
        }))
    }
}

pub fn execute(engine: &Engine, inputs: &[Vector; 16]) -> Result<[Vector; 16], String> {
    let mut m = Machine {
        engine,
        inputs,
        temporary: [[0.; 4]; 16],
        output: [[0.; 4]; 16],
        address: [0; 3],
    };
    let mut pc = (engine.reg(0x48a) & 0xffff) as usize;
    let mut flags = [false; 2];
    let mut stack = Vec::new();
    for _ in 0..16384 {
        if let Some(frame) = stack.last().copied() {
            match frame {
                Frame::Call { end, back } | Frame::If { end, back } if pc == end => {
                    stack.pop();
                    pc = back;
                    continue;
                }
                Frame::Loop {
                    end,
                    back,
                    remaining,
                    increment,
                    previous,
                } if pc == end => {
                    stack.pop();
                    if remaining > 1 {
                        m.address[2] = m.address[2].wrapping_add(increment);
                        stack.push(Frame::Loop {
                            end,
                            back,
                            remaining: remaining - 1,
                            increment,
                            previous,
                        });
                        pc = back;
                    } else {
                        m.address[2] = previous;
                    }
                    continue;
                }
                _ => {}
            }
        }
        let word = *engine
            .code
            .get(pc)
            .ok_or_else(|| format!("DMP shader PC {pc} outside upload"))?;
        let op = word >> 26;
        let target = (word >> 10 & 4095) as usize;
        let count = (word & 255) as usize;
        let uniform = engine.reg(0x480) & (1 << (word >> 22 & 15)) != 0;
        if stack.len() > 32 {
            return Err("DMP shader flow stack exceeds bound".into());
        }
        match op {
            0x21 => {
                pc += 1;
                continue;
            }
            0x22 => {
                if m.output.iter().flatten().any(|v| !v.is_finite()) {
                    return Err("DMP nonfinite shader output is unsupported".into());
                }
                return Ok(m.output);
            }
            0x24..=0x26 => {
                let take = op == 0x24
                    || if op == 0x25 {
                        condition(word, flags)
                    } else {
                        uniform
                    };
                if take {
                    if count == 0 || target + count > engine.code.len() {
                        return Err("DMP shader call extent invalid".into());
                    }
                    stack.push(Frame::Call {
                        end: target + count,
                        back: pc + 1,
                    });
                    pc = target;
                } else {
                    pc += 1;
                }
                continue;
            }
            0x27 | 0x28 => {
                if target < pc + 1 || target + count > engine.code.len() {
                    return Err("DMP shader IF extent invalid".into());
                }
                if if op == 0x27 {
                    uniform
                } else {
                    condition(word, flags)
                } {
                    stack.push(Frame::If {
                        end: target,
                        back: target + count,
                    });
                    pc += 1;
                } else {
                    pc = target;
                }
                continue;
            }
            0x29 => {
                let params = engine.reg(0x481 + (word >> 22 & 3) as u16);
                if target < pc + 1 || target >= engine.code.len() {
                    return Err("DMP shader LOOP extent invalid".into());
                }
                let previous = m.address[2];
                m.address[2] = (params >> 8 & 255) as i32;
                stack.push(Frame::Loop {
                    end: target + 1,
                    back: pc + 1,
                    remaining: (params & 255) + 1,
                    increment: (params >> 16 & 255) as i32,
                    previous,
                });
                pc += 1;
                continue;
            }
            0x2c | 0x2d => {
                let take = if op == 0x2c {
                    condition(word, flags)
                } else {
                    uniform != (word & 1 != 0)
                };
                pc = if take { target } else { pc + 1 };
                continue;
            }
            _ => {}
        }
        let mad = op >= 0x30;
        let inverted = (0x30..0x38).contains(&op) || matches!(op, 0x18..=0x1b);
        let descriptor = *engine
            .operands
            .get((word & if mad { 31 } else { 127 }) as usize)
            .ok_or("DMP shader descriptor outside upload")?;
        let relative = word >> if mad { 22 } else { 19 } & 3;
        let (ai, bi, ci) = if mad {
            (
                word >> 17 & 31,
                word >> if inverted { 12 } else { 10 } & if inverted { 31 } else { 127 },
                word >> 5 & if inverted { 127 } else { 31 },
            )
        } else if inverted {
            (word >> 14 & 31, word >> 7 & 127, 0)
        } else {
            (word >> 12 & 127, word >> 7 & 31, 0)
        };
        let a = m.source(
            ai,
            if !mad && !inverted { relative } else { 0 },
            descriptor >> 5 & 255,
            descriptor & 16 != 0,
        )?;
        let b = m.source(
            bi,
            if inverted && !mad || mad && !inverted {
                relative
            } else {
                0
            },
            descriptor >> 14 & 255,
            descriptor & 8192 != 0,
        )?;
        if op & !1 == 0x2e {
            flags = [
                compare(a[0], b[0], word >> 24 & 7)?,
                compare(a[1], b[1], word >> 21 & 7)?,
            ];
            pc += 1;
            continue;
        }
        if op == 0x12 {
            for (i, value) in a.iter().take(2).enumerate() {
                if descriptor & (8 >> i) != 0 {
                    if !value.is_finite() || value.abs() > 65535. {
                        return Err("DMP shader address conversion outside bound".into());
                    }
                    m.address[i] = value.trunc() as i32;
                }
            }
            pc += 1;
            continue;
        }
        let c = if mad {
            m.source(
                ci,
                if inverted { relative } else { 0 },
                descriptor >> 23 & 255,
                descriptor & (1 << 22) != 0,
            )?
        } else {
            [0.; 4]
        };
        let value = if mad {
            std::array::from_fn(|i| a[i] * b[i] + c[i])
        } else {
            match op {
                0 => std::array::from_fn(|i| a[i] + b[i]),
                1 | 2 => {
                    let n = if op == 1 { 3 } else { 4 };
                    let mut dot = 0.;
                    for i in 0..n {
                        dot += a[i] * b[i];
                    }
                    [dot; 4]
                }
                3 | 0x18 => {
                    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + b[3];
                    [dot; 4]
                }
                4 | 0x19 => [1., a[1] * b[1], a[2], b[3]],
                5 => [libm::exp2f(a[0]); 4],
                6 => [libm::log2f(a[0]); 4],
                8 => std::array::from_fn(|i| a[i] * b[i]),
                9 | 0x1a => std::array::from_fn(|i| if a[i] >= b[i] { 1. } else { 0. }),
                10 | 0x1b => std::array::from_fn(|i| if a[i] < b[i] { 1. } else { 0. }),
                11 => a.map(f32::floor),
                12 => std::array::from_fn(|i| a[i].max(b[i])),
                13 => std::array::from_fn(|i| a[i].min(b[i])),
                14 => [1. / a[0]; 4],
                15 => [1. / libm::sqrtf(a[0]); 4],
                19 => a,
                _ => return Err(format!("DMP unsupported shader opcode {op:#x} at {pc}")),
            }
        };
        let destination = word >> if mad { 24 } else { 21 } & 31;
        let output = if destination < 16 {
            &mut m.output[destination as usize]
        } else {
            &mut m.temporary[destination as usize - 16]
        };
        for i in 0..4 {
            if descriptor & (8 >> i) != 0 {
                output[i] = value[i];
            }
        }
        pc += 1;
    }
    Err("DMP shader execution exceeds instruction bound".into())
}
