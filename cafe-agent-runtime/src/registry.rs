use cafe_sdk::AgentDefinition;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct AgentEntry {
    pub def: AgentDefinition,
    #[allow(dead_code)]
    pub path: PathBuf,
    #[allow(dead_code)]
    pub file_hash: String,
}

pub struct AgentRegistry {
    agents: HashMap<String, AgentEntry>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    pub fn insert(&mut self, entry: AgentEntry) {
        self.agents.insert(entry.def.name.clone(), entry);
    }

    #[allow(dead_code)]
    pub fn get(&self, name: &str) -> Option<&AgentEntry> {
        self.agents.get(name)
    }

    #[allow(dead_code)]
    pub fn get_hash(&self, name: &str) -> &str {
        self.agents
            .get(name)
            .map(|e| e.file_hash.as_str())
            .unwrap_or("")
    }

    #[allow(dead_code)]
    pub fn all(&self) -> impl Iterator<Item = &AgentEntry> {
        self.agents.values()
    }

    #[allow(dead_code)]
    pub fn update(&mut self, name: &str, def: AgentDefinition, hash: String) {
        if let Some(entry) = self.agents.get_mut(name) {
            entry.def = def;
            entry.file_hash = hash;
        }
    }
}
