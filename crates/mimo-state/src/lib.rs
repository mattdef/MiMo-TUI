use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result};

pub mod branch;
pub mod diagnostics_store;
pub mod mcp_store;
pub mod memory_store;
pub mod models;
pub mod session_store;
pub mod skill_store;
pub mod task_store;

pub use models::{AppMode, FileAttachment, PlanItem};

pub(crate) fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

/// Écrit `contents` dans `path` de manière atomique (quand possible).
///
/// # Stratégie
///
/// 1. Écrit dans un fichier temporaire `.{filename}.tmp`
/// 2. Renomme le fichier temporaire vers le chemin cible
///
/// # Limitations
///
/// ## Unix (Linux, macOS)
/// L'opération est atomique grâce à `rename()` qui remplace atomiquement la cible.
///
/// ## Windows
/// L'opération **n'est pas strictement atomique** : `rename()` échoue si la cible existe,
/// donc on doit d'abord supprimer la cible (`remove_file`) puis renommer. Si le processus
/// crashe entre ces deux opérations, le fichier cible est perdu.
///
/// Une vraie atomicité sur Windows nécessiterait `MoveFileEx` avec `MOVEFILE_REPLACE_EXISTING`,
/// mais cela ajouterait une dépendance sur `windows-sys` pour un cas marginal.
///
/// # Garantie
///
/// Sur Unix : le fichier cible est soit l'ancien contenu, soit le nouveau contenu, jamais
/// un état intermédiaire ou corrompu.
///
/// Sur Windows : en cas de crash pendant l'écriture, le fichier cible peut être perdu.
/// Ce risque est acceptable car les fichiers d'état sont régénérables.
pub(crate) fn write_string_atomic(path: &Path, contents: &str) -> Result<()> {
    ensure_parent_dir(path)?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let temp = path.with_file_name(format!(".{file_name}.tmp"));

    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp)
            .with_context(|| format!("failed to open {}", temp.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write {}", temp.display()))?;
        file.flush()
            .with_context(|| format!("failed to flush {}", temp.display()))?;
    }

    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("failed to replace {}", path.display()))?;
    }

    fs::rename(&temp, path)
        .with_context(|| format!("failed to rename {} to {}", temp.display(), path.display()))
}
