//! Read-only process exit observation. Retain a handle before termination so
//! the exit code is available even after the PID disappears or is reused.
use crate::{config, native, service::Shared};
use anyhow::{Result, bail};
use serde_json::json;
use windows::Win32::{Foundation::*, System::Threading::*};

pub struct ExitWatch {
    process: native::Process,
    role: &'static str,
    reported: bool,
}
impl ExitWatch {
    pub fn open(pid: u32, created: Option<f64>, role: &'static str) -> Result<Self> {
        let process = native::Process {
            handle: native::Handle(unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )?
            }),
            pid,
        };
        if created.is_some_and(|c| (process.creation().unwrap_or(0.) - c).abs() > 0.01) {
            bail!("退出观察的进程身份已改变");
        }
        Ok(Self {
            process,
            role,
            reported: false,
        })
    }
    fn exit_code(&mut self) -> Result<Option<u32>> {
        if self.reported {
            return Ok(None);
        }
        match unsafe { WaitForSingleObject(self.process.handle.0, 0) } {
            WAIT_TIMEOUT => return Ok(None),
            WAIT_OBJECT_0 => (),
            _ => return Err(windows::core::Error::from_win32().into()),
        }
        let mut code = 0;
        unsafe {
            GetExitCodeProcess(self.process.handle.0, &mut code)?;
        }
        self.reported = true;
        Ok(Some(code))
    }
    pub fn poll(&mut self, shared: &Shared) {
        match self.exit_code() {
            Ok(Some(code)) => {
                let (mut created, mut exited, mut kernel, mut user) = (
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                );
                let exit_time = unsafe {
                    GetProcessTimes(
                        self.process.handle.0,
                        &mut created,
                        &mut exited,
                        &mut kernel,
                        &mut user,
                    )
                }
                .ok()
                .map(|_| {
                    (((exited.dwHighDateTime as u64) << 32) | exited.dwLowDateTime as u64) as f64
                        / 10_000_000.
                        - 11_644_473_600.
                })
                .filter(|t| *t > 0.);
                shared.event("runtime", &json!({
                    "text":format!("{}进程退出：PID={}，退出码={}（0x{code:08X}），实际结束时间={}；退出码本身不能确定发起者",self.role,self.process.pid,code,exit_time.map(crate::telemetry::source_timestamp).unwrap_or_else(||"未取得".into())),
                    "event":"process_exit","role":self.role,"pid":self.process.pid,"exit_code":code,
                    "exit_code_hex":format!("0x{code:08X}"),"process_exited_at":exit_time
                }));
            }
            Ok(None) => (),
            Err(e) => {
                self.reported = true;
                shared.log(&format!("退出观察失败：{}，{e}", self.role));
            }
        }
    }
}
pub fn note(c: &config::Config, event: &str, text: &str) {
    crate::telemetry::append(
        &c.path.parent().unwrap().join("data"),
        "runtime",
        &json!({"event":event,"text":text,"pid":std::process::id()}),
        c.token(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_exit_code_after_process_disappears_once() {
        use std::{os::windows::process::CommandExt, process::Command};
        let mut child = Command::new("cmd.exe")
            .args(["/d", "/c", "exit /b 23"])
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let mut watch = ExitWatch::open(child.id(), None, "测试").unwrap();
        child.wait().unwrap();
        let d = tempfile::tempdir().unwrap();
        let c = config::Config::default(d.path().join("config.json"));
        let (shared, _) = Shared::new(c);
        watch.poll(&shared);
        watch.poll(&shared);
        let lines = std::fs::read_to_string(d.path().join("data/events.jsonl")).unwrap();
        let events: Vec<serde_json::Value> = lines
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["detail"]["exit_code"], 23);
        assert!(events[0]["detail"]["process_exited_at"].as_f64().unwrap() > config::now() - 5.);
        assert_eq!(events[0]["detail"]["event"], "process_exit");
        assert!(ExitWatch::open(std::process::id(), Some(0.), "测试").is_err());
    }
}
