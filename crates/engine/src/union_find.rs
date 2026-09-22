use std::collections::HashMap;

/// Standard union-find (disjoint set) over indices `0..n`, used wherever the
/// engine needs connected components over a graph of pairwise relationships
/// (reconcile's path-collision groups, LSH candidate buckets, version
/// families).
pub(crate) struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    pub(crate) fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    pub(crate) fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    pub(crate) fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent[ra] = rb;
        }
    }

    /// All connected components, including singletons. Callers that only
    /// want components with more than one member should filter the result.
    pub(crate) fn groups(&mut self) -> Vec<Vec<usize>> {
        let mut map: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..self.parent.len() {
            let root = self.find(i);
            map.entry(root).or_default().push(i);
        }
        map.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_every_element_in_its_own_group() {
        let mut uf = UnionFind::new(3);
        let mut groups = uf.groups();
        groups.sort();
        assert_eq!(groups, vec![vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn union_merges_two_groups() {
        let mut uf = UnionFind::new(4);
        uf.union(0, 1);
        uf.union(2, 3);
        let mut groups = uf.groups();
        for g in &mut groups {
            g.sort();
        }
        groups.sort();
        assert_eq!(groups, vec![vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn union_is_transitive() {
        let mut uf = UnionFind::new(3);
        uf.union(0, 1);
        uf.union(1, 2);
        assert_eq!(uf.find(0), uf.find(2));
    }
}
