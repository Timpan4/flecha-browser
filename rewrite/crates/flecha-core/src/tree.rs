pub(crate) trait Branch: Sized {
    fn children(&self) -> Option<&[Self]>;
    fn children_mut(&mut self) -> Option<&mut Vec<Self>>;
}

pub(crate) fn walk<N: Branch>(nodes: &[N]) -> impl Iterator<Item = (&N, usize, Option<&N>)> {
    let mut frames = vec![(nodes.iter(), None)];
    std::iter::from_fn(move || {
        loop {
            let depth = frames.len().checked_sub(1)?;
            let (siblings, parent) = frames.last_mut()?;
            if let Some(node) = siblings.next() {
                let parent = *parent;
                if let Some(children) = node.children() {
                    frames.push((children.iter(), Some(node)));
                }
                return Some((node, depth, parent));
            }
            frames.pop();
        }
    })
}

pub(crate) fn clone_root<N: Branch>(root: &N, mut shallow: impl FnMut(&N) -> N) -> N {
    let mut tree = Builder::default();
    for (node, depth, _) in walk(std::slice::from_ref(root)) {
        tree.push(depth, shallow(node));
    }
    tree.finish().pop().expect("cloning one root yields a root")
}

pub(crate) fn drop_descendants<N: Branch>(node: &mut N) {
    if let Some(children) = node.children_mut() {
        let mut stack = std::mem::take(children);
        while let Some(mut node) = stack.pop() {
            if let Some(children) = node.children_mut() {
                stack.append(children);
            }
        }
    }
}

pub(crate) struct Builder<N> {
    parents: Vec<(Vec<N>, N)>,
    siblings: Vec<N>,
}

impl<N> Default for Builder<N> {
    fn default() -> Self {
        Self {
            parents: Vec::new(),
            siblings: Vec::new(),
        }
    }
}

impl<N: Branch> Builder<N> {
    pub(crate) fn accepts_depth(&self, depth: usize) -> bool {
        depth <= self.parents.len()
    }

    pub(crate) fn push(&mut self, depth: usize, node: N) {
        self.ascend(depth);
        if node.children().is_some() {
            self.parents
                .push((std::mem::take(&mut self.siblings), node));
        } else {
            self.siblings.push(node);
        }
    }

    pub(crate) fn finish(mut self) -> Vec<N> {
        self.ascend(0);
        self.siblings
    }

    pub(crate) fn ascend(&mut self, depth: usize) {
        while self.parents.len() > depth {
            let (siblings, mut parent) = self.parents.pop().expect("an ancestor remains");
            let children = parent
                .children_mut()
                .expect("only branches can be ancestors");
            *children = std::mem::replace(&mut self.siblings, siblings);
            self.siblings.push(parent);
        }
    }
}
