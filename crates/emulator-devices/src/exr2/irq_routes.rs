//! Interrupt outputs of the EXR II DMA and image blocks, and the routing of
//! their lines to the core whose interrupt controller enables them.
use std::sync::{Arc, Mutex};

/// Physical interrupt output. The board supplies the receiving core; the
/// runtime routes the physical line through that core's interrupt controller.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Irq {
    pub core: usize,
    pub line: u32,
}
impl Irq {
    pub fn raise(self, cx: &mut crate::Cx<'_>) -> Result<(), String> {
        cx.raise_irq(self.core, self.line)
    }
}

/// The interrupt controllers publish their register-derived enables. A unique
/// enable establishes a DMA bank, retained while both controllers mask the
/// line. Ambiguous enables and a bank that was never selected remain unsupported.
/// Graph wiring wakes consumers on route changes and source edges.
#[derive(Clone)]
pub struct IrqRoutes(Arc<Mutex<Routing>>);
#[derive(serde::Serialize, serde::Deserialize)]
struct Routing {
    #[serde(with = "crate::checkpoint::array")]
    owners: [Option<usize>; 128],
    #[serde(with = "crate::checkpoint::array")]
    banks: [Option<usize>; 128],
    enabled: [[u32; 4]; 2],
}
impl Default for IrqRoutes {
    fn default() -> Self {
        let result = Self(Arc::new(Mutex::new(Routing {
            owners: [None; 128],
            banks: [None; 128],
            enabled: [[0; 4]; 2],
        })));
        drop(result.0.lock().unwrap());
        result
    }
}
impl IrqRoutes {
    pub fn checkpoint(&self) -> Result<crate::checkpoint::Checkpoint, String> {
        crate::checkpoint::Checkpoint::encode(
            "image-irq-routes",
            &*self.0.lock().map_err(|_| "image-irq-routes poisoned")?,
        )
    }
    pub fn validate_checkpoint(&self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        let _: Routing = saved.decode("image-irq-routes")?;
        Ok(())
    }
    pub fn restore_checkpoint(&self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        *self.0.lock().map_err(|_| "image-irq-routes poisoned")? =
            saved.decode::<Routing>("image-irq-routes")?;
        Ok(())
    }

    pub fn fingerprint(&self) -> serde_json::Value {
        let state = self.0.lock().unwrap();
        serde_json::json!({"owners":state.owners.as_slice(),"banks":state.banks.as_slice(),"enabled":state.enabled})
    }
    pub fn set(&self, line: u32, owner: Option<usize>) -> Result<(), String> {
        if line >= 128 || owner.is_some_and(|v| v > 1) {
            return Err("invalid image IRQ route".into());
        }
        let mut routes = self.0.lock().unwrap();
        routes.owners[line as usize] = owner;
        if let Some(core) = owner {
            routes.banks[line as usize] = Some(core);
        }
        for core in 0..2 {
            let word = &mut routes.enabled[core][line as usize / 32];
            *word = (*word & !(1 << (line % 32))) | (u32::from(owner == Some(core)) << (line % 32));
        }
        Ok(())
    }
    pub fn owner(&self, line: u32) -> Option<usize> {
        self.0
            .lock()
            .unwrap()
            .owners
            .get(line as usize)
            .copied()
            .flatten()
    }
    pub fn latch(&self, line: u32) -> Result<Irq, String> {
        let routes = self.0.lock().map_err(|_| "image-irq-routes poisoned")?;
        let index = line as usize;
        if index >= 128 {
            return Err("invalid image IRQ line".into());
        }
        let masked = routes
            .enabled
            .iter()
            .all(|words| words[index / 32] & (1 << (line % 32)) == 0);
        let core = routes.owners[index]
            .or_else(|| masked.then_some(routes.banks[index]).flatten())
            .ok_or_else(|| format!("image DMA IRQ {line:#x} needs one programmed owner"))?;
        Ok(Irq { line, core })
    }
    /// Validate a bank selected at START. Masking both controllers does not
    /// cancel the transfer or change that bank; delivery is gated by the
    /// receiving controller. Migration and ambiguous enables still refuse.
    pub fn validate(&self, route: Irq) -> Result<(), String> {
        let routes = self.0.lock().map_err(|_| "image-irq-routes poisoned")?;
        let line = route.line as usize;
        if line >= 128 || route.core >= 2 {
            return Err("invalid latched DMA IRQ".into());
        }
        let mask = 1u32 << (line % 32);
        let masked = routes
            .enabled
            .iter()
            .all(|words| words[line / 32] & mask == 0);
        if routes.owners[line] == Some(route.core) || masked {
            Ok(())
        } else {
            Err("image DMA IRQ owner changed during transfer".into())
        }
    }
}

/// One controller's physical enable-state output, joined with its peer's
/// output. A mask preserves the established bank; ambiguous ownership refuses.
pub struct RouteOutput {
    routes: IrqRoutes,
    core: usize,
}
impl RouteOutput {
    pub fn new(routes: IrqRoutes, core: usize) -> Result<Self, String> {
        if core > 1 {
            return Err("image route output targets an absent EXR II controller".into());
        }
        Ok(Self { routes, core })
    }
}
impl crate::exr2::irq::EnableOutput for RouteOutput {
    fn publish(&self, enabled: [u32; 4]) {
        let mut routes = self.routes.0.lock().unwrap();
        if routes.enabled[self.core] == enabled {
            return;
        }
        routes.enabled[self.core] = enabled;
        for line in 0..128 {
            let bit = 1 << (line % 32);
            routes.owners[line] = match (
                routes.enabled[0][line / 32] & bit != 0,
                routes.enabled[1][line / 32] & bit != 0,
            ) {
                (true, false) => Some(0),
                (false, true) => Some(1),
                _ => None,
            };
            if let Some(core) = routes.owners[line] {
                routes.banks[line] = Some(core);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn masked_rearm_preserves_the_established_bank_through_checkpoint() {
        use crate::exr2::irq::EnableOutput;
        for core in 0..2 {
            let routes = IrqRoutes::default();
            let output = RouteOutput::new(routes.clone(), core).unwrap();
            let mut enabled = [0; 4];
            enabled[2] = 1 << (0x5c % 32);
            assert!(routes.latch(0x5c).is_err());
            output.publish(enabled);
            assert_eq!(routes.latch(0x5c).unwrap().core, core);
            output.publish([0; 4]);
            assert_eq!(routes.owner(0x5c), None);
            let saved = routes.checkpoint().unwrap();
            let identity = routes.fingerprint();
            let restored = IrqRoutes::default();
            restored.restore_checkpoint(&saved).unwrap();
            assert_eq!(restored.fingerprint(), identity);
            let irq = restored.latch(0x5c).unwrap();
            assert_eq!(irq.core, core);
            restored.validate(irq).unwrap();
            restored.set(0x5c, Some(1 - core)).unwrap();
            assert!(restored.validate(irq).is_err());
            assert_eq!(restored.latch(0x5c).unwrap().core, 1 - core);
        }
    }
    #[test]
    fn ambiguous_enables_refuse_rearm_even_with_an_established_bank() {
        use crate::exr2::irq::EnableOutput;
        let routes = IrqRoutes::default();
        let outputs = [
            RouteOutput::new(routes.clone(), 0).unwrap(),
            RouteOutput::new(routes.clone(), 1).unwrap(),
        ];
        let mut enabled = [0; 4];
        enabled[1] = 1 << 12;
        outputs[0].publish(enabled);
        let irq = routes.latch(44).unwrap();
        outputs[1].publish(enabled);
        assert!(routes.latch(44).is_err());
        assert!(routes.validate(irq).is_err());
        outputs[0].publish([0; 4]);
        assert_eq!(routes.latch(44).unwrap().core, 1);
        assert!(routes.latch(128).is_err());
    }
}
