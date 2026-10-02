//! `ledgit merge`: printing a merge preview, and reading choices off the
//! command line.

use crate::show::{amt, truncate};
use ledgit_core::merge::{Case, Row};
use ledgit_core::prelude::*;

/// Apply `--set KEY=CHOICE` to `choices`. KEY is a row's uid (or a unique
/// prefix of it), `group-N`, an issuer's uid prefix or name, or `setting-N`.
pub fn set(p: &MergePreview, choices: &mut Choices, spec: &str) -> Result<()> {
    let (key, value) = spec
        .split_once('=')
        .ok_or_else(|| Error::Invalid(format!("--set wants KEY=CHOICE, not \"{spec}\"")))?;
    let (key, value) = (key.trim(), value.trim().to_lowercase());
    let bad = |what: &str, allowed: &str| {
        Error::Invalid(format!("{what} takes {allowed}, not \"{value}\""))
    };

    if let Some(n) = key.strip_prefix("group-") {
        let g = index(n, p.groups.len(), "group")?;
        let c = match value.as_str() {
            "force" => GroupChoice::Force,
            "revert" => GroupChoice::Revert,
            "drop" => GroupChoice::Drop,
            _ => return Err(bad(key, "force, revert or drop")),
        };
        choices.groups.insert(g, c);
        return Ok(());
    }
    if let Some(n) = key.strip_prefix("setting-") {
        let item = &p.settings[index(n, p.settings.len(), "setting")?];
        let take = match value.as_str() {
            "theirs" | "branch" | "take" => true,
            "ours" | "main" | "leave" => false,
            _ => return Err(bad(key, "theirs or ours")),
        };
        choices.settings.insert(item.key.clone(), take);
        return Ok(());
    }
    let lower = key.to_lowercase();
    let issuers: Vec<_> = p
        .issuers
        .iter()
        .filter(|i| i.name.eq_ignore_ascii_case(key) || i.uid.to_string().starts_with(&lower))
        .collect();
    let rows: Vec<&Row> =
        p.rows.iter().filter(|r| r.tx.uid.to_string().starts_with(&lower)).collect();
    match (rows.as_slice(), issuers.as_slice()) {
        ([row], []) => {
            let c = match (row.side, value.as_str()) {
                (_, "keep") => RowChoice::Keep,
                (Side::Destination, "revert") => RowChoice::Revert,
                (Side::Source, "drop") => RowChoice::Drop,
                (Side::Destination, _) => return Err(bad(key, "keep or revert")),
                (Side::Source, _) => return Err(bad(key, "keep or drop")),
            };
            choices.rows.insert(row.tx.uid, c);
            Ok(())
        }
        ([], [item]) => {
            let c = match value.as_str() {
                "bring" | "both" | "keep" => IssuerChoice::Bring,
                "leave" | "ours" | "main" => IssuerChoice::Leave,
                "take" | "theirs" | "branch" => IssuerChoice::TakeBranch,
                _ => return Err(bad(key, "bring, leave or take")),
            };
            choices.issuers.insert(item.uid, c);
            Ok(())
        }
        ([], []) => {
            Err(Error::Invalid(format!("\"{key}\" is not a row, group, issuer or setting")))
        }
        _ => Err(Error::Invalid(format!("\"{key}\" matches more than one thing; give more of it"))),
    }
}

fn index(n: &str, len: usize, what: &str) -> Result<usize> {
    match n.parse::<usize>() {
        Ok(k) if (1..=len).contains(&k) => Ok(k - 1),
        _ => Err(Error::Invalid(format!("there is no {what}-{n}"))),
    }
}

/// The whole preview, with what each thing is set to under `choices`.
pub fn print(p: &MergePreview, choices: &Choices) {
    println!("{} {} into the current branch", p.kind.name(), p.source);
    if p.fast_forward {
        println!(
            "\nNothing has happened here since {} split off: this just moves up to it.",
            p.source
        );
        return;
    }
    if p.is_empty() {
        println!("\nThe two sides already agree. Nothing to merge.");
        return;
    }
    let name = |uid: LedgerUid| {
        let b = p.branch();
        match (p.destination().ledgers.ix(uid), b.ledgers.ix(uid)) {
            (Some(ix), _) => p.destination().ledgers.name[ix.get()].clone(),
            (None, Some(ix)) => format!("{} (new)", b.ledgers.name[ix.get()]),
            _ => uid.short(),
        }
    };
    let rows = |case: Case, title: &str| {
        let list: Vec<usize> = (0..p.rows.len()).filter(|k| p.rows[*k].case == case).collect();
        if list.is_empty() {
            return;
        }
        println!("\n{title}");
        for k in list {
            line(p, k, choices);
        }
    };
    rows(Case::DestinationOnly, "Only here, on ledgers the branch left alone (keep | revert):");
    rows(Case::SourceOnly, "Only on the branch, on ledgers left alone here (keep | drop):");
    for (g, group) in p.groups.iter().enumerate() {
        let ledgers: Vec<String> = group.ledgers.iter().map(|u| name(*u)).collect();
        let state = match choices.groups.get(&g) {
            _ if p.kind == MergeKind::Replace => "the branch's side".into(),
            Some(c) => format!("{c:?}").to_lowercase(),
            None if group.rows.iter().all(|k| p.choice(*k, choices).is_some()) => {
                "decided row by row".into()
            }
            None => "UNDECIDED".into(),
        };
        println!(
            "\ngroup-{}: both sides changed {} - force | revert | drop  [{state}]",
            g + 1,
            ledgers.join(", ")
        );
        for k in &group.rows {
            line(p, *k, choices);
        }
    }
    if !p.issuers.is_empty() {
        println!("\nIssuers from the branch (bring | leave | take):");
        for i in &p.issuers {
            let state = match p.issuer_choice(i, choices) {
                Some(c) => format!("{c:?}").to_lowercase(),
                None => "UNDECIDED".into(),
            };
            let clash = if i.clashes.is_empty() {
                String::new()
            } else {
                let names: Vec<String> = i
                    .clashes
                    .iter()
                    .filter_map(|u| p.destination().issuers.ix(*u))
                    .map(|ix| p.destination().issuers.name[ix.get()].clone())
                    .collect();
                format!("  - posts to the same ledgers as {}", names.join(", "))
            };
            println!("  {:<10} {:<28} [{state}]{clash}", i.uid.short(), truncate(&i.name, 28));
        }
    }
    if !p.settings.is_empty() {
        println!("\nSettings the branch changed (theirs | ours):");
        for (k, s) in p.settings.iter().enumerate() {
            let state = if p.takes(s, choices) { "theirs" } else { "ours" };
            let both = if s.both_changed { "  (changed here too)" } else { "" };
            println!(
                "  setting-{:<3} {:<36} {} -> {}  [{state}]{both}",
                k + 1,
                truncate(&s.label, 36),
                truncate(&s.ours, 24),
                truncate(&s.theirs, 24)
            );
        }
    }
}

fn line(p: &MergePreview, k: usize, choices: &Choices) {
    let r = &p.rows[k];
    let side = match r.side {
        Side::Destination => "here  ",
        Side::Source => "branch",
    };
    let state = match p.choice(k, choices) {
        Some(c) => format!("{c:?}").to_lowercase(),
        None => "-".into(),
    };
    let note = if r.catch_up { "  (its issuer, run to catch up)" } else { "" };
    println!(
        "  {:<10} {}  {}  {:<28} {:>12}  [{state}]{note}",
        r.tx.uid.short(),
        side,
        r.tx.date,
        truncate(&r.tx.name, 28),
        amt(r.tx.amount())
    );
}
