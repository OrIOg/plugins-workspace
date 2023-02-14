// Copyright 2021 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use tauri::{
    plugin::{Builder, TauriPlugin},
    FsScopeEvent, Manager, Runtime,
};

use std::{
    collections::HashSet,
    fs::{create_dir_all, File},
    io::Write,
    path::PathBuf,
    sync::Mutex,
};

const SCOPE_STATE_FILENAME: &str = ".persisted-scope";

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
    #[error(transparent)]
    TauriApi(#[from] tauri::api::Error),
    #[error(transparent)]
    Bincode(#[from] Box<bincode::ErrorKind>),
}

#[derive(Debug, Default, Deserialize, Serialize, Eq, PartialEq, Hash)]
enum TargetType {
    #[default]
    File,
    Directory,
    RecursiveDirectory,
}

#[derive(Debug, Default, Deserialize, Serialize, Eq, PartialEq, Hash)]
struct ScopePath {
    path: String,
    target_type: TargetType,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Scope {
    allowed_paths: HashSet<ScopePath>,
    forbidden_paths: HashSet<ScopePath>,
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("persisted-scope")
        .setup(|app| {
            let fs_scope = app.fs_scope();
            #[cfg(feature = "protocol-asset")]
            let asset_protocol_scope = app.asset_protocol_scope();
            let app = app.clone();
            let app_dir = app.path_resolver().app_data_dir();

            if let Some(app_dir) = app_dir {
                let scope_state_path = app_dir.join(SCOPE_STATE_FILENAME);

                let _ = fs_scope.forbid_file(&scope_state_path);
                #[cfg(feature = "protocol-asset")]
                let _ = asset_protocol_scope.forbid_file(&scope_state_path);

                let mut scope: Scope = Scope::default();
                if scope_state_path.exists() {
                    scope = tauri::api::file::read_binary(&scope_state_path)
                        .map_err(Error::from)
                        .and_then(|scope| bincode::deserialize(&scope).map_err(Into::into))
                        .unwrap_or_default();

                    for allowed in &scope.allowed_paths {
                        let path = &allowed.path;
                        match allowed.target_type {
                            TargetType::File => {
                                let _ = fs_scope.allow_file(path);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.allow_file(path);
                            }
                            TargetType::Directory => {
                                let _ = fs_scope.allow_directory(path, false);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.allow_directory(path, false);
                            }
                            TargetType::RecursiveDirectory => {
                                let _ = fs_scope.allow_directory(path, true);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.allow_directory(path, true);
                            }
                        }
                    }

                    for forbidden in &scope.forbidden_paths {
                        let path = &forbidden.path;
                        match forbidden.target_type {
                            TargetType::File => {
                                let _ = fs_scope.allow_file(path);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.forbid_file(path);
                            }
                            TargetType::Directory => {
                                let _ = fs_scope.forbid_directory(path, false);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.forbid_directory(path, false);
                            }
                            TargetType::RecursiveDirectory => {
                                let _ = fs_scope.forbid_directory(path, true);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.forbid_directory(path, true);
                            }
                        }
                    }
                }

                let fs_scope_closure = fs_scope.clone();
                let add_to_list = move |path: &PathBuf, list: &mut HashSet<ScopePath>| -> bool {
                    let data = fs_scope_closure.allowed_path_metadata(path.as_path());
                    match data {
                        Some(metadata) => {
                            let scope_path = ScopePath {
                                path: path.to_string_lossy().to_string(),
                                target_type: if metadata.is_dir() {
                                    if metadata.recursive() {
                                        TargetType::RecursiveDirectory
                                    } else {
                                        TargetType::Directory
                                    }
                                } else {
                                    TargetType::File
                                },
                            };
                            list.insert(scope_path);
                            true
                        }
                        None => false,
                    }
                };

                let mutex_scope = Mutex::new(scope);
                fs_scope.listen(move |event| {
                    let lock = mutex_scope.lock();
                    if let Ok(mut scope) = lock {
                        let is_ok = match event {
                            FsScopeEvent::PathAllowed(allowed_path) => {
                                add_to_list(allowed_path, &mut scope.allowed_paths)
                            }
                            FsScopeEvent::PathForbidden(forbidden_path) => {
                                add_to_list(forbidden_path, &mut scope.forbidden_paths)
                            }
                        };

                        if is_ok {
                            let scope_state_path = scope_state_path.clone();

                            let _ = create_dir_all(&app_dir)
                                .and_then(|_| File::create(scope_state_path))
                                .map_err(Error::Io)
                                .and_then(|mut f| {
                                    f.write_all(
                                        &bincode::serialize(&(*scope)).map_err(Error::from)?,
                                    )
                                    .map_err(Into::into)
                                });
                        }
                    } else {
                        println!("try_lock failed");
                    }
                });
            }
            Ok(())
        })
        .build()
}
