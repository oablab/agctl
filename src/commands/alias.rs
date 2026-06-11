use anyhow::Result;
use crate::AliasAction;
use crate::config::AliasStore;

pub fn handle(action: AliasAction) -> Result<()> {
    match action {
        AliasAction::Set { name, arn } => {
            let mut store = AliasStore::load();
            store.aliases.insert(name.clone(), arn.clone());
            store.save()?;
            println!("✅ {name} → {arn}");
            Ok(())
        }
        AliasAction::List => {
            let store = AliasStore::load();
            if store.aliases.is_empty() {
                println!("No aliases configured.");
            } else {
                for (name, arn) in &store.aliases {
                    println!("{name:<20} {arn}");
                }
            }
            Ok(())
        }
        AliasAction::Remove { name } => {
            let mut store = AliasStore::load();
            store.aliases.remove(&name);
            store.save()?;
            println!("Removed alias '{name}'");
            Ok(())
        }
    }
}
