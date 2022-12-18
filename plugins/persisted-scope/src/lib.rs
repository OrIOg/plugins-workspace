// Copyright 2021 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use tauri::{
    plugin::{Builder, TauriPlugin},
    FsScopeEvent, GlobPattern, Manager, Runtime,
};

use std::{
    collections::HashSet,
    fs::{create_dir_all, File},
    io::Write,
    path::{PathBuf, MAIN_SEPARATOR},
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

                    println!("{:#?}", scope);

                    for allowed in scope.allowed_paths.iter() {
                        let path = &allowed.path;
                        match allowed.target_type {
                            TargetType::File => {
                                let _ = fs_scope.allow_file(&path);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.allow_file(path);
                            }
                            TargetType::Directory => {
                                let _ = fs_scope.allow_directory(path, false);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.allow_directory(path, false);
                            }
                            TargetType::RecursiveDirectory => {
                                let _ = fs_scope.allow_directory(&path, true);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.allow_directory(path, true);
                            }
                        }
                    }

                    for allowed in scope.forbidden_paths.iter() {
                        let path = &allowed.path;
                        match allowed.target_type {
                            TargetType::File => {
                                let _ = fs_scope.allow_file(&path);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.forbid_file(path);
                            }
                            TargetType::Directory => {
                                let _ = fs_scope.forbid_directory(&path, false);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.forbid_directory(path, false);
                            }
                            TargetType::RecursiveDirectory => {
                                let _ = fs_scope.forbid_directory(&path, true);
                                #[cfg(feature = "protocol-asset")]
                                let _ = asset_protocol_scope.forbid_directory(path, true);
                            }
                        }
                    }
                }

                fn scope_path_from_patterns(
                    path: &PathBuf,
                    patterns: &HashSet<GlobPattern>,
                ) -> ScopePath {
                    let path = path.to_string_lossy();
                    let mut target_type = TargetType::File;
                    for pattern in patterns {
                        let escaped_path = GlobPattern::escape(&path);
                        let pre_pattern = format!("{}{}{}", escaped_path, MAIN_SEPARATOR, '*');
                        let str_pattern = pattern.to_string();
                        if str_pattern.contains(&pre_pattern) {
                            target_type = if str_pattern.ends_with("**") {
                                TargetType::RecursiveDirectory
                            } else {
                                TargetType::Directory
                            };
                        }
                    }
                    return ScopePath {
                        path: path.to_string(),
                        target_type,
                    };
                }

                let mutex_scope = Mutex::new(scope);
                fs_scope.listen(move |event| {
                    let lock = mutex_scope.lock();
                    if let Ok(mut scope) = lock {
                        match event {
                            FsScopeEvent::PathAllowed(allowed_path) => {
                                let scope_path = scope_path_from_patterns(
                                    allowed_path,
                                    &app.fs_scope().allowed_patterns(),
                                );
                                scope.allowed_paths.insert(scope_path);
                            }
                            FsScopeEvent::PathForbidden(forbidden_path) => {
                                let scope_path = scope_path_from_patterns(
                                    forbidden_path,
                                    &app.fs_scope().forbidden_patterns(),
                                );
                                scope.forbidden_paths.insert(scope_path);
                            }
                        };

                        let scope_state_path = scope_state_path.clone();

                        let _ = create_dir_all(&app_dir)
                            .and_then(|_| File::create(scope_state_path))
                            .map_err(Error::Io)
                            .and_then(|mut f| {
                                f.write_all(&bincode::serialize(&(*scope)).map_err(Error::from)?)
                                    .map_err(Into::into)
                            });
                    } else {
                        println!("try_lock failed");
                    }
                });
            }
            Ok(())
        })
        .build()
}
