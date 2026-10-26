//! Opt-in host CPU observations, excluded from all modeled state and decisions.
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

pub(super) type CpuCounters = BTreeMap<&'static str, u64>;

fn now() -> Result<Duration, String> {
    #[cfg(unix)]
    {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // A per-thread CPU clock excludes descheduling and other sessions.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0 {
            return Err(format!(
                "thread CPU clock: {}",
                std::io::Error::last_os_error()
            ));
        }
        if time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) {
            return Err("thread CPU clock returned an invalid duration".into());
        }
        Ok(Duration::new(time.tv_sec as u64, time.tv_nsec as u32))
    }
    #[cfg(not(unix))]
    Err("host-profile requires a Unix thread CPU clock".into())
}

#[derive(Clone, Copy)]
pub(super) struct Stamp {
    at: Duration,
    devices: Duration,
}
#[derive(Default)]
struct Row {
    calls: u64,
    inclusive: Duration,
    exclusive: Duration,
}
#[derive(Default)]
pub(super) struct Profile {
    cpu_baseline: Vec<CpuCounters>,
    devices: BTreeMap<(usize, &'static str), Row>,
    engines: BTreeMap<usize, Row>,
    device_cpu: Duration,
    run_device_cpu: Duration,
    run_cpu: Duration,
    run_depth: usize,
}
impl Profile {
    pub fn new(cpu_baseline: Vec<CpuCounters>) -> Result<Self, String> {
        now()?;
        Ok(Self {
            cpu_baseline,
            ..Self::default()
        })
    }
    pub fn start(&self) -> Stamp {
        self.start_at(now().expect("thread CPU clock failed after profiling started"))
    }
    fn start_at(&self, at: Duration) -> Stamp {
        Stamp {
            at,
            devices: self.device_cpu,
        }
    }
    pub fn begin_run(&mut self) -> Stamp {
        let stamp = self.start();
        self.run_depth += 1;
        stamp
    }
    pub fn finish_run(&mut self, stamp: Stamp) {
        self.finish_run_at(
            stamp,
            now().expect("thread CPU clock failed during execution"),
        );
    }
    fn finish_run_at(&mut self, stamp: Stamp, at: Duration) {
        self.run_depth -= 1;
        if self.run_depth == 0 {
            self.run_cpu += at - stamp.at;
        }
    }
    pub fn record(&mut self, device: usize, phase: &'static str, stamp: Stamp) {
        self.record_at(
            device,
            phase,
            stamp,
            now().expect("thread CPU clock failed during device call"),
        );
    }
    fn record_at(&mut self, device: usize, phase: &'static str, stamp: Stamp, at: Duration) {
        let inclusive = at - stamp.at;
        let child = self.device_cpu - stamp.devices;
        let exclusive = inclusive
            .checked_sub(child)
            .expect("nested device CPU exceeds its parent");
        let row = self.devices.entry((device, phase)).or_default();
        row.calls += 1;
        row.inclusive += inclusive;
        row.exclusive += exclusive;
        self.device_cpu += exclusive;
        if self.run_depth != 0 {
            self.run_device_cpu += exclusive;
        }
    }
    pub fn record_engine(&mut self, core: usize, stamp: Stamp) {
        self.record_engine_at(
            core,
            stamp,
            now().expect("thread CPU clock failed during CPU execution"),
        );
    }
    fn record_engine_at(&mut self, core: usize, stamp: Stamp, at: Duration) {
        let inclusive = at - stamp.at;
        let devices = self.device_cpu - stamp.devices;
        let row = self.engines.entry(core).or_default();
        row.calls += 1;
        row.inclusive += inclusive;
        row.exclusive += inclusive
            .checked_sub(devices)
            .expect("MMIO device CPU exceeds engine CPU");
    }
    pub fn report(
        &self,
        name: impl Fn(usize) -> &'static str,
        counters: &[Result<CpuCounters, String>],
    ) -> Value {
        let engine: Duration = self.engines.values().map(|r| r.exclusive).sum();
        // Dispatch, callbacks outside device bodies, clock reads and profiler
        // bookkeeping remain visible rather than being assigned to a device.
        let other = self.run_cpu.checked_sub(engine + self.run_device_cpu);
        json!({"clock":"thread-cpu",
            "run_cpu_seconds":self.run_cpu.as_secs_f64(),
            "engine_cpu_seconds":engine.as_secs_f64(),
            "device_cpu_seconds":self.device_cpu.as_secs_f64(),
            "run_device_cpu_seconds":self.run_device_cpu.as_secs_f64(),
            "runtime_cpu_seconds":other.map(|v|v.as_secs_f64()),
            "engine_definition":"CPU backend and callbacks excluding measured device bodies",
            "cpu_exit_counters":counters.iter().enumerate().map(|(core,current)|match current {
                Ok(current)=>json!({"core":core,"delta":current.iter().map(|(&name,&value)| {
                    let delta=self.cpu_baseline.get(core).and_then(|b|b.get(name))
                        .and_then(|&before|value.checked_sub(before));
                    (name,delta)
                }).collect::<BTreeMap<_,_>>()}),
                Err(error)=>json!({"core":core,"error":error}),
            }).collect::<Vec<_>>(),
            "engines":self.engines.iter().map(|(&core,r)|json!({"core":core,"calls":r.calls,
                "inclusive_cpu_seconds":r.inclusive.as_secs_f64(),"cpu_seconds":r.exclusive.as_secs_f64()})).collect::<Vec<_>>(),
            "devices":self.devices.iter().map(|(&(device,phase),r)|json!({"device":device,
                "kind":name(device),"phase":phase,"calls":r.calls,
                "cpu_seconds":r.exclusive.as_secs_f64(),"inclusive_cpu_seconds":r.inclusive.as_secs_f64()})).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    #[test]
    fn nested_mmio_is_excluded_from_engine_and_parent_device_time() {
        let mut p = Profile::default();
        p.run_depth = 1;
        let run = p.start_at(ms(0));
        let engine = p.start_at(ms(10));
        let parent = p.start_at(ms(15));
        let child = p.start_at(ms(16));
        p.record_at(2, "read", child, ms(18));
        p.record_at(1, "write", parent, ms(25));
        p.record_engine_at(0, engine, ms(30));
        p.finish_run_at(run, ms(40));
        let report = p.report(|_| "device", &[]);
        assert_eq!(report["engine_cpu_seconds"], 0.010);
        assert_eq!(report["run_device_cpu_seconds"], 0.010);
        assert_eq!(report["runtime_cpu_seconds"], 0.020);
        assert_eq!(p.devices[&(1, "write")].exclusive, ms(8));
        assert_eq!(p.devices[&(2, "read")].exclusive, ms(2));
        assert_eq!(p.devices[&(1, "write")].inclusive, ms(10));
    }
    #[test]
    fn device_observations_outside_execution_do_not_pollute_run_residual() {
        let mut p = Profile::default();
        let stamp = p.start_at(ms(0));
        p.record_at(1, "read", stamp, ms(5));
        assert_eq!(p.device_cpu, ms(5));
        assert_eq!(p.run_device_cpu, Duration::ZERO);
        assert_eq!(p.report(|_| "device", &[])["runtime_cpu_seconds"], 0.0);
    }
    #[test]
    fn cpu_counts_are_phase_deltas_and_reports_do_not_change_the_baseline() {
        let p = Profile {
            cpu_baseline: vec![BTreeMap::from([
                ("worker_rpc_run", 40),
                ("worker_callback_events", 9),
                ("reset_counter", 5),
            ])],
            ..Profile::default()
        };
        let counters = [
            Ok(BTreeMap::from([
                ("worker_rpc_run", 47),
                ("worker_callback_events", 11),
                ("reset_counter", 1),
            ])),
            Err("CPU unavailable".into()),
        ];
        let report = p.report(|_| "device", &counters);
        assert_eq!(report["cpu_exit_counters"][0]["delta"]["worker_rpc_run"], 7);
        assert_eq!(
            report["cpu_exit_counters"][0]["delta"]["worker_callback_events"],
            2
        );
        assert_eq!(
            report["cpu_exit_counters"][0]["delta"]["reset_counter"],
            Value::Null
        );
        assert_eq!(report["cpu_exit_counters"][1]["error"], "CPU unavailable");
        assert_eq!(report, p.report(|_| "device", &counters));
    }
    #[test]
    #[cfg(unix)]
    fn thread_cpu_clock_is_monotonic() {
        let a = now().unwrap();
        let b = now().unwrap();
        assert!(b >= a);
    }
}
