mod errors;
use super::{Store, frames};
use crate::{
    claude::mac_connection,
    credentials::Provider,
    login_item,
    mac_monitor::presentation::DASHBOARD,
    mac_settings::{self, Account, AccountId, Appearance, Edit, MenuDisplay, Storage, Target},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use objc2_foundation::NSBundle;
use serde::Deserialize;
use serde_json::json;
use zeroize::Zeroizing;

fn trimmed(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || c == '\u{200b}')
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Command {
    Visibility {
        _0: bool,
    },
    RefreshAll {},
    RefreshAccounts {},
    Refresh {
        _0: AccountId,
    },
    Toggle {
        _0: AccountId,
    },
    Remove {
        _0: AccountId,
    },
    Interval {
        _0: i64,
    },
    AutoRefresh {
        _0: bool,
    },
    AutomaticUpdates {
        _0: bool,
    },
    CheckAppUpdate {},
    InstallAppUpdate {},
    LaunchAtLogin {
        _0: bool,
    },
    RefreshLaunchAtLogin {},
    OpenLoginItemsSettings {},
    Appearance {
        _0: Appearance,
    },
    DockIcon {
        _0: bool,
    },
    MenuBarMode {
        _0: MenuDisplay,
    },
    ScanLogs {
        _0: bool,
    },
    MenuBarVisibility {
        _0: AccountId,
        _1: bool,
    },
    Message {
        _0: Option<String>,
    },
    Connect {
        id: AccountId,
        provider: Provider,
        path: String,
        alias: String,
    },
    CodexLogin {
        id: AccountId,
        data: String,
        alias: String,
        replacing: Option<AccountId>,
    },
}

impl Store {
    fn account(&self, id: AccountId) -> Option<Account> {
        let s = self.state.borrow();
        s.settings.index(id).map(|i| s.settings.accounts[i].clone())
    }

    fn change(&self, edit: Edit, secret: &[u8]) -> Result<(bool, Option<Account>), String> {
        let storage = if !self.healthy {
            Storage::Unavailable
        } else if self.demo {
            Storage::Memory
        } else {
            Storage::Disk
        };
        let current = self.state.borrow().settings.clone();
        let path = self.root.join("settings.json");
        let import = match &edit {
            Edit::ImportLogin { provider, path, .. } => Some((*provider, path.clone())),
            _ => None,
        };
        let result = mac_settings::apply(
            current,
            edit,
            storage,
            &path,
            Some(self.sessions.vault_directory()),
            secret,
        )
        .map_err(|e| errors::message(e, &path, import.as_ref()))?;
        let row = result.index.map(|i| result.settings.accounts[i].clone());
        if result.changed {
            self.state.borrow_mut().settings = result.settings;
            self.schedule();
            self.configure_updates();
            self.changed();
        }
        Ok((result.changed, row))
    }

    fn edit(&self, edit: Edit) -> bool {
        match self.change(edit, &[]) {
            Ok((changed, _)) => changed,
            Err(error) => {
                self.message(Some(error));
                false
            }
        }
    }

    fn connect(&self, provider: Provider, path: &str, alias: &str) -> Result<(), String> {
        if self.demo {
            return Err(
                "미리보기에서는 계정을 저장하지 않습니다. 실제 앱을 열어 연결하세요.".into(),
            );
        }
        if !self.healthy {
            return Err("설정을 먼저 복구하세요.".into());
        }
        let path = mac_connection::canonical(trimmed(path));
        let alias = trimmed(alias).to_owned();
        let edit = if provider == Provider::Claude {
            let helper = NSBundle::mainBundle()
                .executableURL()
                .and_then(|url| url.path())
                .ok_or("앱 실행 파일을 찾을 수 없습니다.")?
                .to_string();
            Edit::BindClaude {
                directory: path,
                helper,
                alias,
            }
        } else {
            Edit::ImportLogin {
                provider,
                path,
                alias,
            }
        };
        if let (_, Some(row)) = self.change(edit, &[])? {
            if provider != Provider::Claude {
                self.sessions.invalidate(row.id.0);
            }
            self.cancel(row.id);
            self.refresh(row.id);
        }
        Ok(())
    }

    fn login(&self, data: &[u8], alias: &str, replacing: Option<AccountId>) -> Result<(), String> {
        let target = replacing
            .map(|id| {
                self.account(id)
                    .map(|a| Target {
                        id: a.id,
                        identity: a.identity,
                    })
                    .ok_or("삭제된 계정입니다. 새 계정으로 연결하세요.")
            })
            .transpose()?;
        if data.len() > 1_048_576 {
            return Err("로그인 정보가 너무 큽니다.".into());
        }
        if self.demo || !self.healthy {
            return Err("현재 상태에서는 계정을 연결할 수 없습니다.".into());
        }
        let alias = trimmed(alias).to_owned();
        if let (_, Some(row)) = self.change(Edit::ManagedLogin { alias, target }, data)? {
            self.sessions.invalidate(row.id.0);
            self.cancel(row.id);
            let mut s = self.state.borrow_mut();
            s.errors.remove(&row.id.0);
            s.notes.remove(&row.id.0);
            drop(s);
            self.refresh(row.id);
        }
        Ok(())
    }

    fn remove(&self, row: Account) -> Result<(), String> {
        if !self.demo && !self.healthy {
            return Err("설정을 먼저 복구하세요.".into());
        }
        let id = row.id;
        let edit = if !self.demo && row.provider == Provider::Claude {
            Edit::RemoveClaude {
                id,
                identity: row.identity,
            }
        } else {
            Edit::Remove {
                id,
                delete_credential: row.provider != Provider::Claude,
            }
        };
        self.change(edit, &[])?;
        self.sessions.invalidate(id.0);
        self.cancel(id);
        let mut s = self.state.borrow_mut();
        s.snapshots.remove(&id.0);
        s.errors.remove(&id.0);
        s.notes.remove(&id.0);
        drop(s);
        self.changed();
        Ok(())
    }

    fn reply(&self, id: AccountId, result: Result<(), String>) {
        if let Some(p) = self.presentation.upgrade() {
            if result.is_ok() {
                p.send(DASHBOARD, &self.snapshot(true));
            }
            p.send(DASHBOARD, &frames::reply(id, result.err().as_deref()));
        }
    }

    pub(in crate::mac_monitor) fn command(&self, data: &[u8]) -> i32 {
        use Command::*;
        let Ok(cmd) = serde_json::from_slice(data) else {
            return -1;
        };
        match cmd {
            Visibility { _0: v } => self.set_visible(v),
            RefreshAll {} | RefreshAccounts {} => self.refresh_all(),
            Refresh { _0: id } => self.refresh(id),
            Toggle { _0: id } => {
                if self.edit(Edit::Toggle { id })
                    && let Some(row) = self.account(id)
                {
                    if row.enabled {
                        self.refresh(id)
                    } else {
                        self.cancel(id)
                    }
                }
            }
            Remove { _0: id } => {
                if let Some(row) = self.account(id)
                    && let Err(e) = self.remove(row)
                {
                    self.message(Some(e));
                }
            }
            Interval { _0: seconds } => {
                self.edit(Edit::Interval { seconds });
            }
            AutoRefresh { _0: enabled } => {
                self.edit(Edit::AutoRefresh { enabled });
            }
            AutomaticUpdates { _0: enabled } => {
                self.edit(Edit::AutomaticUpdates { enabled });
            }
            CheckAppUpdate {} => {
                if self.demo {
                    self.state.borrow_mut().update = json!({"phase":"idle","message":"미리보기에서는 업데이트를 내려받거나 설치하지 않습니다."});
                    self.changed();
                } else if let Some(u) = self.updater.get() {
                    u.check();
                }
            }
            InstallAppUpdate {} => {
                if let Some(u) = self.updater.get() {
                    u.install_now();
                }
            }
            LaunchAtLogin { _0: on } => {
                if self.demo {
                    self.state.borrow_mut().login = if on {
                        login_item::State::Enabled
                    } else {
                        login_item::State::Disabled
                    };
                    self.changed();
                } else {
                    if login_item::macos::state() == login_item::State::Unavailable {
                        self.message(Some(
                            "빌드된 QuotaBar.app에서 자동 실행을 설정하세요.".into(),
                        ));
                    } else if login_item::macos::set(on).is_err() {
                        self.message(Some("자동 실행 설정을 변경하지 못했습니다. 시스템 설정의 로그인 항목을 확인하세요.".into()));
                    }
                    self.refresh_login();
                }
            }
            RefreshLaunchAtLogin {} => self.refresh_login(),
            OpenLoginItemsSettings {} => {
                if !self.demo {
                    login_item::macos::open_settings();
                }
            }
            Appearance { _0: mode } => {
                if self.edit(Edit::Appearance { mode }) {
                    self.apply_appearance();
                }
            }
            DockIcon { _0: visible } => {
                if self.edit(Edit::ShowDockIcon { visible }) {
                    self.apply_dock();
                }
            }
            MenuBarMode { _0: mode } => {
                self.edit(Edit::MenuBarDisplayMode { mode });
            }
            ScanLogs { _0: enabled } => {
                if self.edit(Edit::ScanLocalLogs { enabled }) && self.demo && !enabled {
                    self.state.borrow_mut().report =
                        Some(json!({"days":[],"scannedFiles":0,"warnings":[]}));
                }
            }
            MenuBarVisibility {
                _0: id,
                _1: visible,
            } => {
                self.edit(Edit::MenuVisibility { id, visible });
            }
            Message { _0: text } => self.message(text),
            Connect {
                id,
                provider,
                path,
                alias,
            } => self.reply(id, self.connect(provider, &path, &alias)),
            CodexLogin {
                id,
                data,
                alias,
                replacing,
            } => {
                let encoded = Zeroizing::new(data);
                let Ok(decoded) = STANDARD.decode(encoded.as_bytes()) else {
                    return -1;
                };
                let secret = Zeroizing::new(decoded);
                self.reply(id, self.login(&secret, &alias, replacing));
            }
        }
        0
    }
}
