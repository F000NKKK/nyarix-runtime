//! Dependency graph construction from parsed manifests (see issue #53).
//!
//! **Scope note:** nodes here are package *names*, not name+version —
//! deciding which concrete version of a dependency actually satisfies a
//! manifest's declared [`semver::VersionReq`] is the Version resolver's
//! job (#54), not this graph's. This graph only needs names to detect
//! cycles and compute a load order; #54 refines the edges this produces
//! into version-pinned ones. Likewise, a dependency name with no
//! matching package isn't treated as an error here — see
//! [`DependencyGraph::missing_dependencies`] — because whether that's
//! fatal depends on whether the dependency was declared optional (#56)
//! and is Conflict detection's call (#55), not this issue's.

use std::collections::HashMap;

use nyarix_package::PackageManifest;

/// A directed graph of "package A depends on package B" edges, built
/// from manifests' `[dependencies]` tables.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    /// Package name -> the dependency names it declares.
    edges: HashMap<String, Vec<String>>,
}

/// A dependency cycle was found while computing a load order.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("dependency cycle detected: {}", cycle.join(" -> "))]
pub struct DependencyCycle {
    /// The package names forming the cycle, in traversal order, with the
    /// first name repeated at the end (e.g. `["a", "b", "c", "a"]` for
    /// `a -> b -> c -> a`).
    pub cycle: Vec<String>,
}

#[derive(Default)]
enum Mark {
    #[default]
    Unvisited,
    InProgress,
    Done,
}

impl DependencyGraph {
    /// Build a graph from a set of manifests, one node per
    /// `manifest.package.name`.
    pub fn build<'a>(manifests: impl IntoIterator<Item = &'a PackageManifest>) -> Self {
        let edges = manifests
            .into_iter()
            .map(|manifest| {
                let deps = manifest.dependencies.keys().cloned().collect();
                (manifest.package.name.clone(), deps)
            })
            .collect();
        Self { edges }
    }

    /// Every distinct package name in this graph.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.edges.keys().map(String::as_str)
    }

    /// `(package, dependency)` pairs where `dependency` is declared by
    /// `package` but has no matching node in this graph — i.e. no known
    /// package provides it.
    ///
    /// Not an error by itself: see this module's scope note.
    #[must_use]
    pub fn missing_dependencies(&self) -> Vec<(String, String)> {
        let mut missing: Vec<(String, String)> = self
            .edges
            .iter()
            .flat_map(|(name, deps)| {
                deps.iter()
                    .filter(|dep| !self.edges.contains_key(*dep))
                    .map(move |dep| (name.clone(), dep.clone()))
            })
            .collect();
        missing.sort();
        missing
    }

    /// Compute a load order in which every package appears after all of
    /// its (known) dependencies — a topological sort.
    ///
    /// Edges to a name with no matching node (see
    /// [`Self::missing_dependencies`]) are skipped rather than followed,
    /// since there's nothing to visit.
    ///
    /// # Errors
    /// Returns [`DependencyCycle`] if the graph isn't a DAG.
    pub fn load_order(&self) -> Result<Vec<String>, DependencyCycle> {
        let mut marks: HashMap<&str, Mark> = self
            .edges
            .keys()
            .map(|name| (name.as_str(), Mark::default()))
            .collect();
        let mut order = Vec::new();
        let mut path = Vec::new();

        let mut names: Vec<&str> = self.edges.keys().map(String::as_str).collect();
        names.sort_unstable();

        for name in names {
            self.visit(name, &mut marks, &mut path, &mut order)?;
        }

        Ok(order)
    }

    fn visit<'a>(
        &'a self,
        name: &'a str,
        marks: &mut HashMap<&'a str, Mark>,
        path: &mut Vec<&'a str>,
        order: &mut Vec<String>,
    ) -> Result<(), DependencyCycle> {
        match marks.get(name) {
            Some(Mark::Done) => return Ok(()),
            Some(Mark::InProgress) => {
                let start = path.iter().position(|&n| n == name).unwrap_or(0);
                let mut cycle: Vec<String> =
                    path[start..].iter().map(|s| (*s).to_string()).collect();
                cycle.push(name.to_string());
                return Err(DependencyCycle { cycle });
            }
            _ => {}
        }

        marks.insert(name, Mark::InProgress);
        path.push(name);

        if let Some(deps) = self.edges.get(name) {
            let mut deps: Vec<&str> = deps.iter().map(String::as_str).collect();
            deps.sort_unstable();
            for dep in deps {
                if self.edges.contains_key(dep) {
                    self.visit(dep, marks, path, order)?;
                }
            }
        }

        path.pop();
        marks.insert(name, Mark::Done);
        order.push(name.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str, deps: &[&str]) -> PackageManifest {
        let deps_toml: String = deps.iter().map(|d| format!("{d} = \"^0.1\"\n")).collect();
        let toml = format!(
            r#"
[package]
name = "{name}"
version = "0.1.0"
module_type = "flow"
api_version = "1.0"
author = "Nyarix"
description = "test"

[dependencies]
{deps_toml}
"#
        );
        PackageManifest::from_toml(&toml).unwrap()
    }

    #[test]
    fn dependencies_load_before_dependents() {
        let a = manifest("a", &["b"]);
        let b = manifest("b", &[]);
        let graph = DependencyGraph::build([&a, &b]);

        let order = graph.load_order().unwrap();
        let a_pos = order.iter().position(|n| n == "a").unwrap();
        let b_pos = order.iter().position(|n| n == "b").unwrap();
        assert!(b_pos < a_pos, "b must load before a: {order:?}");
    }

    #[test]
    fn detects_a_direct_cycle() {
        let a = manifest("a", &["b"]);
        let b = manifest("b", &["a"]);
        let graph = DependencyGraph::build([&a, &b]);

        let err = graph.load_order().unwrap_err();
        assert!(err.cycle.contains(&"a".to_string()));
        assert!(err.cycle.contains(&"b".to_string()));
    }

    #[test]
    fn detects_a_longer_cycle() {
        let a = manifest("a", &["b"]);
        let b = manifest("b", &["c"]);
        let c = manifest("c", &["a"]);
        let graph = DependencyGraph::build([&a, &b, &c]);

        let err = graph.load_order().unwrap_err();
        assert_eq!(err.cycle.len(), 4);
    }

    #[test]
    fn a_self_dependency_is_a_cycle() {
        let a = manifest("a", &["a"]);
        let graph = DependencyGraph::build([&a]);

        let err = graph.load_order().unwrap_err();
        assert_eq!(err.cycle, vec!["a".to_string(), "a".to_string()]);
    }

    #[test]
    fn missing_dependencies_are_reported_not_errored() {
        let a = manifest("a", &["ghost"]);
        let graph = DependencyGraph::build([&a]);

        assert!(graph.load_order().is_ok());
        assert_eq!(
            graph.missing_dependencies(),
            vec![("a".to_string(), "ghost".to_string())]
        );
    }

    #[test]
    fn a_diamond_dependency_resolves_cleanly() {
        // d depends on b and c, both of which depend on a.
        let a = manifest("a", &[]);
        let b = manifest("b", &["a"]);
        let c = manifest("c", &["a"]);
        let d = manifest("d", &["b", "c"]);
        let graph = DependencyGraph::build([&a, &b, &c, &d]);

        let order = graph.load_order().unwrap();
        let pos = |name: &str| order.iter().position(|n| n == name).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn independent_packages_have_no_forced_relative_order_but_all_appear() {
        let a = manifest("a", &[]);
        let b = manifest("b", &[]);
        let graph = DependencyGraph::build([&a, &b]);

        let order = graph.load_order().unwrap();
        assert_eq!(order.len(), 2);
        assert!(order.contains(&"a".to_string()));
        assert!(order.contains(&"b".to_string()));
    }
}
