//! 프로세스 목록과 Discord 프로세스 트리 찾기.

use std::collections::{HashMap, HashSet};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

/// 스트림 오디오에서 빼야 하는 Discord 실행 파일 이름(대소문자 무시).
pub const DISCORD_EXE_NAMES: [&str; 3] = ["Discord.exe", "DiscordPTB.exe", "DiscordCanary.exe"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcEntry {
    pub pid: u32,
    pub parent: u32,
    pub exe: String,
}

/// 실행 중인 프로세스 목록(Toolhelp 스냅숏). 실패하면 빈 목록.
pub(crate) fn snapshot_processes() -> Vec<ProcEntry> {
    let mut out = Vec::new();
    // SAFETY: 스냅숏 핸들은 아래에서 닫는다. 구조체 크기를 채워 넘긴다.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                out.push(ProcEntry {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    exe: wide_to_string(&entry.szExeFile),
                });
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

pub(crate) fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

pub(crate) fn is_discord_exe(exe: &str) -> bool {
    DISCORD_EXE_NAMES
        .iter()
        .any(|n| n.eq_ignore_ascii_case(exe))
}

/// Discord 루트 프로세스: Discord 실행 파일이면서 부모가 Discord 프로세스가 아닌 것.
///
/// 자손이 많은 순서(동률이면 PID 순)로 돌려준다. 보통 하나이고,
/// 안정판과 PTB를 함께 켰다면 둘 이상이다.
pub(crate) fn discord_roots(procs: &[ProcEntry]) -> Vec<u32> {
    let discord: HashSet<u32> = procs
        .iter()
        .filter(|p| is_discord_exe(&p.exe))
        .map(|p| p.pid)
        .collect();
    let mut roots: Vec<u32> = procs
        .iter()
        .filter(|p| discord.contains(&p.pid) && (!discord.contains(&p.parent) || p.parent == p.pid))
        .map(|p| p.pid)
        .collect();
    let counts: HashMap<u32, usize> = roots
        .iter()
        .map(|&r| (r, descendant_count(procs, r)))
        .collect();
    roots.sort_by(|a, b| counts[b].cmp(&counts[a]).then(a.cmp(b)));
    roots
}

fn descendant_count(procs: &[ProcEntry], root: u32) -> usize {
    let mut seen = HashSet::from([root]);
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        for c in procs.iter().filter(|e| e.parent == p && e.pid != p) {
            if seen.insert(c.pid) {
                stack.push(c.pid);
            }
        }
    }
    seen.len() - 1
}

/// 실행 중인 Discord(안정판·PTB·Canary)의 루트 프로세스 PID 목록.
///
/// 루트는 "Discord 실행 파일이면서 부모가 Discord 프로세스가 아닌" 프로세스다.
/// 자식 프로세스(렌더러, 음성 유틸리티 등)는 루트의 트리에 포함되므로
/// 프로세스 루프백 EXCLUDE에 루트 PID 하나를 넘기면 Discord 소리 전체가 빠진다.
/// 자손이 많은 루트가 앞에 온다. Discord가 없으면 빈 목록이다.
pub fn find_discord_root_pids() -> Vec<u32> {
    discord_roots(&snapshot_processes())
}

/// 프로세스 실행 파일 이름(예: `chrome.exe`). 권한이 없거나 종료되었으면 `None`.
pub(crate) fn process_name(pid: u32) -> Option<String> {
    // SAFETY: 핸들은 아래에서 닫는다. 버퍼 길이를 정확히 넘긴다.
    unsafe {
        let handle: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        res.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit(['\\', '/']).next().map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, exe: &str) -> ProcEntry {
        ProcEntry {
            pid,
            parent,
            exe: exe.to_string(),
        }
    }

    #[test]
    fn finds_discord_roots() {
        let procs = vec![
            p(4, 0, "System"),
            p(100, 4, "explorer.exe"),
            p(200, 100, "Discord.exe"), // 루트
            p(201, 200, "Discord.exe"),
            p(202, 200, "discord.exe"),
            p(203, 202, "Discord.exe"),
            p(300, 999, "DiscordPTB.exe"), // 부모가 없어진 루트
            p(301, 300, "DiscordPTB.exe"),
            p(400, 100, "chrome.exe"),
        ];
        assert_eq!(discord_roots(&procs), vec![200, 300]);
        assert!(discord_roots(&[p(1, 0, "notepad.exe")]).is_empty());
    }

    #[test]
    fn discord_names_case_insensitive() {
        assert!(is_discord_exe("DISCORDCANARY.EXE"));
        assert!(!is_discord_exe("DiscordSetup.exe"));
    }

    #[test]
    fn snapshot_contains_self() {
        let me = std::process::id();
        assert!(snapshot_processes().iter().any(|e| e.pid == me));
        assert!(process_name(me).is_some());
    }
}
