//! HTML → Markdown conversion for the `Auto`/`Markdown` delivery modes
//! (spec §4.5).
//!
//! Conversion is best-effort: any failure, or a converted result that
//! exceeds `output_cap`, degrades to [`None`] so the caller can fall back to
//! inlining the original text, which is already budget-checked. This
//! function never returns an error.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use htmd::HtmlToMarkdown;
use html5ever::interface::tree_builder::{
    ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink,
};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, LocalName, Namespace, ParseOpts, QualName, parse_document};

/// Maximum tolerated element-nesting depth of the **real** DOM `html5ever`
/// builds for a document.
///
/// # Depth convention
///
/// "Depth" here is exactly the quantity the differential test's
/// `real_dom_max_depth` measures against `markup5ever_rcdom`: the number of
/// ancestors an element node has, counting from the `Document` node at
/// depth 0. Because html5ever's tree builder synthesises the implied
/// `<html>`, `<head>` and `<body>` elements, a document written as
/// `<div>` × N has real depth `N + 2` (`html` = 1, `body` = 2, first `div`
/// = 3), *not* N. Earlier revisions of this guard counted only explicitly
/// written elements and so were off by two against the invariant they
/// claimed to uphold; [`exceeds_safe_nesting_depth`] now measures the real
/// tree, so guard depth and real depth are the same number and the
/// comparison (`real > MAX_SCAN_DEPTH` ⇒ trip) is exact.
///
/// 512 is well above anything a legitimate document nests by hand or by
/// templating, and well below the depth at which `htmd`'s recursive DOM
/// walk (and `RcDom`'s construction) risks exhausting the stack.
const MAX_SCAN_DEPTH: usize = 512;

/// Index into [`DepthTree::nodes`]. This is the `TreeSink::Handle` type: a
/// plain integer, never a reference-counted node, which is what keeps the
/// whole measurement free of recursive structures (see
/// [`exceeds_safe_nesting_depth`]).
type NodeId = usize;

/// One node of the flat arena. Holds only integers, a cheap atom-based
/// [`QualName`], and a `Vec<NodeId>` — no owning links to other nodes, so
/// dropping the arena is a flat `Vec` drop with no recursion at any depth.
struct NodeRec {
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// Depth recorded when this node was last attached to a parent. Used
    /// only for the early-exit fast path; [`DepthTree::max_element_depth`]
    /// recomputes depth exactly and is what the trip decision uses whenever
    /// the parse runs to completion.
    depth: usize,
    /// `Some` only for element nodes; `None` for the document root,
    /// comments, processing instructions and template-content roots.
    /// Only element nodes contribute to the measured depth, matching
    /// `real_dom_max_depth`'s `NodeData::Element` filter.
    name: Option<QualName>,
    mathml_annotation_xml_integration_point: bool,
}

/// Owned [`ElemName`] handed back from [`TreeSink::elem_name`].
///
/// `QualName` is three interned atoms, so cloning one out of the arena is
/// cheap and avoids handing out a borrow of the `RefCell` (which html5ever
/// holds across further sink calls).
#[derive(Debug)]
struct OwnedElemName(QualName);

impl ElemName for OwnedElemName {
    fn ns(&self) -> &Namespace {
        &self.0.ns
    }

    fn local_name(&self) -> &LocalName {
        &self.0.local
    }
}

/// The flat arena a [`DepthSink`] writes into, shared with the caller of
/// [`measure_nesting_depth`] through an [`Rc`] so the parse can be abandoned
/// mid-document without needing the sink back out of the parser.
struct DepthTree {
    nodes: RefCell<Vec<NodeRec>>,
    /// Set the moment an element is attached at a depth past
    /// [`MAX_SCAN_DEPTH`]. Purely an early-exit signal: it lets the caller
    /// abandon the parse instead of building the rest of a hostile
    /// document, and it can only ever *over*-trip.
    exceeded: Cell<bool>,
    /// Set when a structural operation is impossible to honour (e.g. a
    /// sibling insertion for a node the tree builder never parented). Used
    /// both to avoid panicking (`RcDom` panics in this situation; this
    /// crate must not) and, since leaving the node unparented would
    /// otherwise silently drop it and its descendants from
    /// `max_element_depth`'s reachability walk — an under-trip — to also
    /// trip [`Self::exceeded`] directly, so an unexpected shape fails
    /// closed. Asserted never to fire in the tests; kept as a distinct flag
    /// (rather than folded into `exceeded`) purely so the tests can tell
    /// the two conditions apart.
    saw_unexpected_shape: Cell<bool>,
}

impl DepthTree {
    fn new() -> Self {
        let root = NodeRec {
            parent: None,
            children: Vec::new(),
            depth: 0,
            name: None,
            mathml_annotation_xml_integration_point: false,
        };
        Self {
            nodes: RefCell::new(vec![root]),
            exceeded: Cell::new(false),
            saw_unexpected_shape: Cell::new(false),
        }
    }

    fn push_node(&self, name: Option<QualName>, integration_point: bool) -> NodeId {
        let mut nodes = self.nodes.borrow_mut();
        nodes.push(NodeRec {
            parent: None,
            children: Vec::new(),
            depth: 0,
            name,
            mathml_annotation_xml_integration_point: integration_point,
        });
        nodes.len() - 1
    }

    /// Record `child`'s depth as `parent_depth + 1` and raise the early-exit
    /// flag if that puts an element past the cap.
    fn record_depth(&self, parent: NodeId, child: NodeId) {
        let mut nodes = self.nodes.borrow_mut();
        let parent_depth = nodes.get(parent).map_or(0, |node| node.depth);
        let depth = parent_depth.saturating_add(1);
        let is_element = match nodes.get_mut(child) {
            Some(node) => {
                node.depth = depth;
                node.name.is_some()
            }
            None => return,
        };
        if is_element && depth > MAX_SCAN_DEPTH {
            self.exceeded.set(true);
        }
    }

    /// Detach `child` from whatever parent it currently has, if any.
    fn detach(&self, child: NodeId) {
        let mut nodes = self.nodes.borrow_mut();
        let Some(parent) = nodes.get(child).and_then(|node| node.parent) else {
            return;
        };
        if let Some(parent_node) = nodes.get_mut(parent) {
            parent_node.children.retain(|&id| id != child);
        }
        if let Some(child_node) = nodes.get_mut(child) {
            child_node.parent = None;
        }
    }

    /// Append `child` as the last child of `parent`.
    fn attach(&self, parent: NodeId, child: NodeId) {
        self.detach(child);
        let mut nodes = self.nodes.borrow_mut();
        if let Some(parent_node) = nodes.get_mut(parent) {
            parent_node.children.push(child);
        }
        if let Some(child_node) = nodes.get_mut(child) {
            child_node.parent = Some(parent);
        }
        drop(nodes);
        self.record_depth(parent, child);
    }

    /// The real maximum element depth of the tree built so far, measured by
    /// an **iterative** walk over an explicit `Vec` stack of integers.
    /// There is deliberately no recursive traversal anywhere in this file.
    fn max_element_depth(&self) -> usize {
        let nodes = self.nodes.borrow();
        let mut max_depth = 0usize;
        let mut stack: Vec<(NodeId, usize)> = vec![(0, 0)];
        while let Some((id, depth)) = stack.pop() {
            let Some(node) = nodes.get(id) else {
                continue;
            };
            if node.name.is_some() {
                max_depth = max_depth.max(depth);
            }
            for &child in &node.children {
                stack.push((child, depth + 1));
            }
        }
        max_depth
    }
}

/// A [`TreeSink`] that records tree *shape* and nothing else.
///
/// html5ever's real `TreeBuilder` drives this sink exactly as it drives
/// `RcDom`, so namespace tracking (foreign content), RCDATA/rawtext
/// tokenizer-state switching, implied end tags, foster parenting and the
/// adoption agency algorithm are all performed **by html5ever**, not
/// approximated here. See [`exceeds_safe_nesting_depth`].
struct DepthSink {
    tree: Rc<DepthTree>,
}

impl TreeSink for DepthSink {
    type Handle = NodeId;
    type Output = Self;
    type ElemName<'a>
        = OwnedElemName
    where
        Self: 'a;

    fn finish(self) -> Self {
        self
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> NodeId {
        0
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> OwnedElemName {
        let nodes = self.tree.nodes.borrow();
        let name = nodes
            .get(*target)
            .and_then(|node| node.name.clone())
            // Unreachable in practice (html5ever only calls this for
            // elements) but this crate denies `panic!`/`expect`, and a
            // synthetic name is harmless: it can only make the tree builder
            // *close* elements it would have kept open, i.e. under-count,
            // and it never fires.
            .unwrap_or_else(|| {
                QualName::new(
                    None,
                    Namespace::from("http://www.w3.org/1999/xhtml"),
                    LocalName::from("div"),
                )
            });
        OwnedElemName(name)
    }

    fn create_element(
        &self,
        name: QualName,
        _attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> NodeId {
        let id = self
            .tree
            .push_node(Some(name), flags.mathml_annotation_xml_integration_point);
        if flags.template {
            // `RcDom` keeps template contents *outside* the child list, so
            // its own depth walk (and `htmd`'s markdown walk) never
            // descends into it. We attach it as an ordinary child instead:
            // that can only over-count, which is the fail-closed direction.
            let contents = self.tree.push_node(None, false);
            self.tree.attach(id, contents);
        }
        id
    }

    fn create_comment(&self, _text: StrTendril) -> NodeId {
        self.tree.push_node(None, false)
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> NodeId {
        self.tree.push_node(None, false)
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        // Text nodes are always leaves, and `real_dom_max_depth` only
        // measures element nodes, so text is dropped rather than
        // materialised. This changes no depth and saves an allocation per
        // text run.
        if let NodeOrText::AppendNode(node) = child {
            self.tree.attach(*parent, node);
        }
    }

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        let NodeOrText::AppendNode(node) = new_node else {
            return;
        };
        let parent = self
            .tree
            .nodes
            .borrow()
            .get(*sibling)
            .and_then(|n| n.parent);
        let Some(parent) = parent else {
            // `RcDom` panics here; this crate must not. This is not a
            // conservative (over-trip) fallback: leaving `node` (and every
            // descendant later attached under it) unparented drops it from
            // `max_element_depth`'s walk entirely, since that walk only
            // reaches nodes reachable from the root. That is an
            // *under*-trip, and the last shape that could defeat the guard.
            // So this branch must fail closed: trip `exceeded` directly
            // rather than merely recording that the shape was seen.
            self.tree.saw_unexpected_shape.set(true);
            self.tree.exceeded.set(true);
            return;
        };
        self.tree.detach(node);
        let mut nodes = self.tree.nodes.borrow_mut();
        if let Some(parent_node) = nodes.get_mut(parent) {
            let index = parent_node
                .children
                .iter()
                .position(|&id| id == *sibling)
                .unwrap_or(parent_node.children.len());
            parent_node.children.insert(index, node);
        }
        if let Some(child_node) = nodes.get_mut(node) {
            child_node.parent = Some(parent);
        }
        drop(nodes);
        self.tree.record_depth(parent, node);
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let has_parent = self
            .tree
            .nodes
            .borrow()
            .get(*element)
            .is_some_and(|node| node.parent.is_some());
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
        // A doctype is a non-element leaf directly under the document; it
        // cannot affect the maximum *element* depth.
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        // The contents node is the template element's first (and, at
        // creation time, only) child; see `create_element`.
        self.tree
            .nodes
            .borrow()
            .get(*target)
            .and_then(|node| node.children.first().copied())
            .unwrap_or(*target)
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn add_attrs_if_missing(&self, _target: &NodeId, _attrs: Vec<Attribute>) {
        // Attributes never affect nesting depth and are not stored.
    }

    fn remove_from_parent(&self, target: &NodeId) {
        self.tree.detach(*target);
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        let moved = {
            let mut nodes = self.tree.nodes.borrow_mut();
            match nodes.get_mut(*node) {
                Some(source) => std::mem::take(&mut source.children),
                None => return,
            }
        };
        let mut nodes = self.tree.nodes.borrow_mut();
        for &child in &moved {
            if let Some(child_node) = nodes.get_mut(child) {
                child_node.parent = Some(*new_parent);
            }
        }
        if let Some(target) = nodes.get_mut(*new_parent) {
            target.children.extend(moved);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &NodeId) -> bool {
        self.tree
            .nodes
            .borrow()
            .get(*target)
            .is_some_and(|node| node.mathml_annotation_xml_integration_point)
    }

    /// `RcDom` deep-clones the `<option>` subtree into a sibling
    /// `<selectedcontent>` element whenever the tree builder decides one
    /// applies (see `markup5ever`'s
    /// `get_a_selects_enabled_selectedcontent`), which duplicates whatever
    /// depth the `<option>` subtree already has. `TreeSink`'s default is a
    /// no-op, which would make that duplicated depth invisible to this
    /// guard -- an under-trip.
    ///
    /// This crate does not attempt to model the clone (reproducing
    /// `RcDom`'s exact selectedcontent-eligibility and deep-copy logic here
    /// would be exactly the kind of parser-shaped code this design was
    /// built to avoid). Instead it fails closed: any call to this hook
    /// trips the guard outright, so a document that exercises this path is
    /// rejected rather than silently under-measured.
    ///
    /// As of `markup5ever_rcdom` 0.38.0 this hook is inert in practice --
    /// an upstream bug in `get_a_selects_enabled_selectedcontent`
    /// destructures the wrong node and always returns `None`, so html5ever
    /// never actually calls it today (confirmed by instrumentation). That
    /// is exactly why this override exists rather than being left as the
    /// no-op default: a patch-level upstream fix to that bug would silently
    /// reintroduce a real clone, and without this override that would be a
    /// seventh under-trip discovered the same way the first six were.
    fn maybe_clone_an_option_into_selectedcontent(&self, _option: &NodeId) {
        self.tree.saw_unexpected_shape.set(true);
        self.tree.exceeded.set(true);
    }
}

/// Pre-parse guard against pathologically deep HTML (F-4).
///
/// `htmd`'s conversion parses the document into an `RcDom` and then walks —
/// and drops — that DOM recursively, so a document with tens or hundreds of
/// thousands of nested elements exhausts the stack and **aborts the
/// process** (SIGABRT) rather than returning an error. This guard runs
/// first and returns `true` when the document's real nesting depth exceeds
/// [`MAX_SCAN_DEPTH`], in which case [`html_to_markdown`] returns `None`
/// and the caller falls back to inlining plain text.
///
/// # Why this measures with the real tree builder
///
/// Five earlier revisions approximated the parse — first with a hand-rolled
/// byte scanner (bypassed by mismatched-close padding, by self-closing
/// non-void tags, by tag-name truncation, by `</div>` inside a comment, and
/// by `NUL` inside a tag name), then with html5ever's bare *tokenizer*
/// (bypassed by `<svg>` + `<input>`×N, where an ordinary start tag in the
/// SVG/MathML namespace stays open because the HTML void-element list does
/// not apply there, and by `<title><!--</title>` + `<div>`×N, where the
/// absence of a tree builder meant the tokenizer never entered RCDATA state
/// and swallowed the whole document as one comment). Every round closed one
/// production rule and left the next one open. The lesson, paid for six
/// times: **an approximation of the parser is not the parser.**
///
/// So this version does not approximate anything. It runs
/// [`parse_document`] — the same entry point, with the same
/// [`ParseOpts`] defaults `htmd` uses (including
/// `TreeBuilderOpts::scripting_enabled = true`) — against a [`DepthSink`]
/// that implements [`TreeSink`] and records tree shape only. Namespace
/// tracking and the foreign-content breakout list, RCDATA/rawtext/script
/// tokenizer-state switching, implied end tags, foster parenting and the
/// adoption agency algorithm are executed by html5ever itself, exactly as
/// they are for `RcDom`. There is no separate model of *HTML's parsing
/// rules* left in this file to diverge from html5ever the way the
/// tokenizer-only and byte-scanner approximations did.
///
/// That is narrower than saying there is no divergence risk at all.
/// [`DepthSink`] is still a hand-written implementation of the `TreeSink`
/// *contract* html5ever drives, alongside `RcDom`'s -- and every hook of
/// that contract this sink gets wrong is exactly the same class of bug that
/// produced bypasses 1–6, just moved one layer up (from "does this
/// approximate HTML" to "does this correctly mirror what `RcDom` does with
/// each tree-mutation callback"). The `maybe_clone_an_option_into_selectedcontent`
/// gap fixed below was precisely that: not a parsing-rule divergence, but a
/// `TreeSink`-contract one. See "Residuals" for the ones known today.
///
/// # Why parsing here is safe when `htmd`'s parse is not
///
/// The danger in `htmd` is not the parse but the `Rc`-linked tree it
/// produces: walking and dropping it recurses once per level.
/// [`DepthSink`] never builds such a structure. Its handles are `usize`
/// indices; its entire state is a flat `Vec<NodeRec>` (see [`DepthTree`]),
/// where each `NodeRec` holds `Option<usize>`, `Vec<usize>`, a `usize`, an
/// atom-based [`QualName`] and a `bool`. No node owns another node, so:
///   - dropping the arena drops a `Vec` of `Vec<usize>` — one flat loop, no
///     recursion, no depth-proportional stack use, and the same is true
///     when the parse is abandoned mid-document;
///   - [`DepthTree::max_element_depth`] walks with an explicit `Vec` stack,
///     never the call stack;
///   - html5ever's own `TreeBuilder` keeps its open-element stack in a
///     `Vec<Handle>` = `Vec<usize>` and is likewise non-recursive.
///
/// So the deepest input this guard can be handed costs memory proportional
/// to the (already capped) input size and constant stack.
///
/// # Early exit
///
/// The document is fed to the parser in [`FEED_CHUNK_BYTES`] chunks. Each
/// attachment records the child's depth as `parent depth + 1`; the moment
/// an element lands past [`MAX_SCAN_DEPTH`] the parse is abandoned and
/// `true` is returned without building the rest of a hostile document. When
/// the parse instead runs to completion, the trip decision comes from
/// [`DepthTree::max_element_depth`], an exact recomputation over the
/// finished tree — so the early-exit bookkeeping can only ever *add* trips,
/// never remove one.
///
/// # Residuals
///
/// Four. None can cause an under-trip; the first and second can only make
/// this guard reject a document `htmd` would have survived, producing a
/// plain-text fallback rather than an error, and the third affects only the
/// (advisory) early-exit estimate:
///   - `<template>` contents are attached as an ordinary child here, while
///     `RcDom` keeps them off the child list (so neither its depth walk nor
///     `htmd`'s markdown walk descends into them).
///   - `TreeSink::reparent_children` (the adoption agency) moves a subtree
///     without rewriting the recorded depths of the nodes below it, so the
///     *early-exit* estimate can be stale afterwards. It is only an
///     estimate: the authoritative number is the exact recomputation above,
///     which reads the final parent/child links and is unaffected.
///   - `TreeSink::append_before_sibling` for a parentless sibling — which
///     `RcDom` treats as a panic-worthy invariant violation this sink must
///     not replicate — trips the guard outright instead of attempting the
///     insertion. This *used* to be a fifth, under-trip-shaped residual
///     (the sink recorded `saw_unexpected_shape` and silently dropped the
///     subtree from the measurement); it is now fail-closed, so it is only
///     ever an over-trip risk, asserted never to fire in the tests.
///   - `TreeSink::maybe_clone_an_option_into_selectedcontent` — the hook
///     `RcDom` uses to deep-clone an `<option>` subtree into a sibling
///     `<selectedcontent>` element — is not modelled here; any call to it
///     trips the guard outright rather than risking an unmeasured clone.
///     As of `markup5ever_rcdom` 0.38.0 an upstream bug means html5ever
///     never actually calls this hook, so it does not fire in practice
///     today, but a future upstream patch could change that, which is why
///     it is handled explicitly rather than left on the (silently
///     no-op) `TreeSink` default.
///
/// The differential test
/// (`differential_guard_never_under_trips_against_the_real_parser`) and the
/// seeded generative test
/// (`generative_fuzz_guard_never_under_trips_against_the_real_parser`)
/// assert the one invariant that matters, against the real
/// `html5ever` + `markup5ever_rcdom` parse: whenever the real DOM's depth
/// exceeds [`MAX_SCAN_DEPTH`], this guard trips.
fn exceeds_safe_nesting_depth(html: &str) -> bool {
    measure_nesting_depth(html).exceeds
}

/// Outcome of one measurement pass. Exposed (rather than folded into a
/// `bool`) so the tests can assert the exact depth at the cap boundary and
/// that the conservative `saw_unexpected_shape` path never fires; production
/// only needs `exceeds`.
#[cfg_attr(not(test), allow(dead_code))]
struct Measurement {
    /// Whether the document's real nesting depth exceeds [`MAX_SCAN_DEPTH`].
    exceeds: bool,
    /// The exact real depth, or `None` when the parse was abandoned early
    /// (in which case `exceeds` is already `true`).
    depth: Option<usize>,
    /// See [`DepthTree::saw_unexpected_shape`].
    saw_unexpected_shape: bool,
}

/// Bytes handed to the parser per `process` call.
///
/// This is the granularity of the early exit, and it matters more than it
/// looks: html5ever's own tree construction is quadratic in nesting depth
/// for common tags (`<div>` runs "has a `p` element in button scope", which
/// scans the open-element stack), so the cost of a hostile chunk grows with
/// the square of how deep it gets. 4 KiB bounds the depth reachable inside
/// one chunk to roughly a thousand levels — far enough past
/// [`MAX_SCAN_DEPTH`] to make the decision, cheap enough that the scan of a
/// 2 MiB `<div>`-bomb finishes in single-digit milliseconds. Measured: at
/// 64 KiB the same input took ~900 ms.
const FEED_CHUNK_BYTES: usize = 4 * 1024;

/// Drive html5ever's real `parse_document` over `html` with a [`DepthSink`],
/// abandoning the parse as soon as an element is attached past the cap.
fn measure_nesting_depth(html: &str) -> Measurement {
    let tree = Rc::new(DepthTree::new());
    let sink = DepthSink {
        tree: Rc::clone(&tree),
    };
    let mut parser = parse_document(sink, ParseOpts::default());

    let mut rest = html;
    let mut abandoned = false;
    while !rest.is_empty() {
        let mut end = FEED_CHUNK_BYTES.min(rest.len());
        while end > 0 && !rest.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            // Only reachable if a single char exceeded the chunk size,
            // which cannot happen for UTF-8; feed the remainder rather
            // than loop forever.
            end = rest.len();
        }
        let (chunk, remainder) = rest.split_at(end);
        parser.process(StrTendril::from_slice(chunk));
        rest = remainder;
        if tree.exceeded.get() {
            abandoned = true;
            break;
        }
    }

    if abandoned {
        // Drop the parser without finishing it. Everything being dropped —
        // the tree builder's `Vec<usize>` open-element stack and this
        // arena's `Vec<NodeRec>` — is flat, so no recursive `Drop` runs.
        drop(parser);
        return Measurement {
            exceeds: true,
            depth: None,
            saw_unexpected_shape: tree.saw_unexpected_shape.get(),
        };
    }

    drop(parser.finish());
    let depth = tree.max_element_depth();
    Measurement {
        exceeds: depth > MAX_SCAN_DEPTH,
        depth: Some(depth),
        saw_unexpected_shape: tree.saw_unexpected_shape.get(),
    }
}

/// Convert `html` to Markdown, dropping scripts/styles/comments.
///
/// Returns `None` when conversion fails, when the input's nesting depth
/// exceeds [`MAX_SCAN_DEPTH`] (see [`exceeds_safe_nesting_depth`], F-4), or
/// when the converted Markdown's byte length exceeds `output_cap` — in
/// every case the caller falls back to inlining the original text.
/// Conversion is expected to run only on bodies already under the raw read
/// cap. Callers pass the same `effective_cap`/`max_result_budget` used by
/// every other inline delivery path here, not `max_result_bytes - 4096`, so
/// there is only one budget concept to reason about.
///
/// Note: `htmd` treats `<title>` as an ordinary block element (only
/// `script`/`style` are skipped), so a document's `<title>` text appears as
/// a leading plain-text line ahead of the rest of the conversion.
pub(crate) fn html_to_markdown(html: &str, output_cap: usize) -> Option<String> {
    if exceeds_safe_nesting_depth(html) {
        return None;
    }
    let converter = HtmlToMarkdown::builder()
        // Explicit even though htmd drops these by default, to keep the
        // guarantee visible at the call site regardless of upstream default
        // changes.
        .skip_tags(vec!["script", "style"])
        .build();
    let markdown = converter.convert(html).ok()?;
    if markdown.len() > output_cap {
        return None;
    }
    Some(markdown)
}

#[cfg(test)]
mod tests {
    use super::html_to_markdown;

    const BIG_CAP: usize = 1_048_576;

    // Goldens are committed with a single trailing newline; `htmd`'s output
    // has none, so every comparison below trims both sides consistently.

    #[test]
    fn nested_lists_match_golden() {
        let html = include_str!("../fixtures/nested_lists.html");
        let want = include_str!("../fixtures/nested_lists.md").trim_end();
        let markdown = html_to_markdown(html, BIG_CAP).expect("conversion succeeds");
        assert_eq!(markdown.trim_end(), want);
    }

    #[test]
    fn table_matches_golden() {
        let html = include_str!("../fixtures/table.html");
        let want = include_str!("../fixtures/table.md").trim_end();
        let markdown = html_to_markdown(html, BIG_CAP).expect("conversion succeeds");
        assert_eq!(markdown.trim_end(), want);
    }

    #[test]
    fn script_and_style_are_dropped() {
        let html = include_str!("../fixtures/script_style.html");
        let want = include_str!("../fixtures/script_style.md").trim_end();
        let markdown = html_to_markdown(html, BIG_CAP).expect("conversion succeeds");
        assert_eq!(markdown.trim_end(), want);
        assert!(!markdown.contains("var secret"));
        assert!(!markdown.contains("color: red"));
        assert!(!markdown.contains("display: none"));
        assert!(!markdown.contains("document.write"));
        assert!(!markdown.contains("inline script marker"));
        assert!(!markdown.contains("a comment that must not appear"));
        assert!(markdown.contains("Visible Heading"));
        assert!(markdown.contains("Visible paragraph text"));
    }

    #[test]
    fn pathological_markup_converts_without_panicking() {
        // html5ever (htmd's parser) is a browser-grade forgiving parser, so
        // this malformed input is not expected to make conversion fail —
        // the assertion here is that it does not panic and produces some
        // output containing the visible text. The `None`-on-cap-exceeded
        // path is covered directly below instead, per the task brief (a
        // genuine conversion *failure* is not reproducible with htmd/
        // html5ever's forgiving parser).
        let html = include_str!("../fixtures/pathological.html");
        let markdown = html_to_markdown(html, BIG_CAP).expect("htmd tolerates malformed markup");
        assert!(markdown.contains("Broken"));
    }

    #[test]
    fn output_cap_exceeded_yields_none() {
        let html = include_str!("../fixtures/nested_lists.html");
        // The full conversion is far larger than one byte; a one-byte cap
        // forces the `None` fallback path deterministically.
        assert!(html_to_markdown(html, 1).is_none());
    }

    /// Build a document with `depth` levels of nested `<div>` wrapping a
    /// short text node, e.g. depth=3 -> `<div><div><div>text</div></div></div>`.
    fn nested_divs(depth: usize) -> String {
        let mut html = String::with_capacity(depth * 11 + 16);
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str("text");
        for _ in 0..depth {
            html.push_str("</div>");
        }
        html
    }

    #[test]
    fn deeply_nested_html_does_not_crash() {
        // F-4: a document with 100_000 levels of nesting must not blow the
        // stack during parse/convert/drop.
        let html = nested_divs(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(result.is_none(), "expected the depth guard to trip");
    }

    #[test]
    fn moderately_nested_html_still_converts() {
        // ~100 levels deep is well under the guard's cap and should convert
        // normally rather than being rejected.
        let html = nested_divs(100);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(result.is_some(), "moderate nesting should still convert");
        assert!(result.unwrap().contains("text"));
    }

    #[test]
    fn mismatched_close_padding_does_not_bypass_the_depth_guard() {
        // Bypass 1 regression: `<div></span>` repeated leaves every `<div>`
        // genuinely open in the real DOM (the `</span>` never matches), so
        // this must still trip the guard, not decrement past it.
        let html = "<div></span>".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip on mismatched-close padding"
        );
    }

    #[test]
    fn self_closing_non_void_padding_does_not_bypass_the_depth_guard() {
        // Bypass 2 regression: `<div/>` repeated stays open in the real DOM
        // (self-closing is ignored on a non-void, non-foreign element).
        let html = "<div/>".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip on self-closing non-void padding"
        );
    }

    #[test]
    fn truncated_void_name_padding_does_not_bypass_the_depth_guard() {
        // Bypass 3 regression: `<img_x>` is a different, non-void element
        // from the void `<img>`, and stays open in the real DOM.
        let html = "<img_x>".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip on truncated-void-name padding"
        );
    }

    #[test]
    fn truncated_close_padding_does_not_bypass_the_depth_guard() {
        // Bypass 3 regression, closing-tag direction: a genuinely open run
        // of `<div>`s followed by padded closes (`</div_x>`) that must not
        // pop them (html5ever ignores `</div_x>` as an unmatched end tag).
        let opens = "<div>".repeat(1_000);
        let padded_closes = "</div_x>".repeat(1_000);
        let html = format!("{opens}{padded_closes}");
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip: padded closes must not pop real opens"
        );
    }

    #[test]
    fn comment_wrapped_close_padding_does_not_bypass_the_depth_guard() {
        // Bypass 4 regression: the scanner had no comment state at all, so
        // `<div><!--</div>-->` repeated let the `</div>` *inside* the
        // comment pop the genuinely open `<div>` right next to it -- the
        // tracked stack oscillated 0<->1 while html5ever's real DOM nested
        // 1000+ deep (every `<div>` stays open; comments are never tags).
        let html = "<div><!--</div>-->".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip: a `</div>` inside a comment must not pop a real div"
        );
    }

    #[test]
    fn nul_padded_close_does_not_bypass_the_depth_guard() {
        // Bypass 5 regression: NUL was wrongly treated as a tag-name
        // terminator. html5ever replaces NUL with U+FFFD *inside* a name
        // (a continuation byte, not a terminator), so `</div\0>` is an
        // unmatched end tag for a name that isn't "div" and must not pop a
        // genuinely open `<div>`.
        let opens = "<div>".repeat(1_000);
        let padded_closes = "</div\u{0}>".repeat(1_000);
        let html = format!("{opens}{padded_closes}");
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip: a NUL-padded close must not pop a real open"
        );
    }

    #[test]
    fn foreign_content_open_padding_does_not_bypass_the_depth_guard() {
        // Bypass 6a regression: inside the SVG/MathML namespace an ordinary
        // start tag with no self-closing flag is inserted and STAYS OPEN.
        // Only names on the foreign-content breakout list (br, img, hr,
        // embed, meta, ...) escape, and `input` is not one of them. The
        // previous tokenizer-based guard applied the *HTML* void-element
        // set to a namespace-less tokenizer name, so it never pushed and
        // the document sailed through into a real SIGABRT.
        let html = format!("<svg>{}", "<input>".repeat(100_000));
        // Asserting the guard directly (not just that conversion produced
        // `None`), since `None` is also produced by an output-cap miss or a
        // genuine `htmd` conversion failure -- neither of which is what
        // this regression is about.
        assert!(
            super::exceeds_safe_nesting_depth(&html),
            "expected the depth guard to trip on foreign-content open padding"
        );
        assert!(html_to_markdown(&html, BIG_CAP).is_none());
    }

    #[test]
    fn rawtext_unterminated_comment_prefix_does_not_bypass_the_depth_guard() {
        // Bypass 6b regression: an 18-byte prefix disabled the whole guard.
        // A bare tokenizer is never put into RCDATA state (the tree builder
        // normally drives that switch), so it read the `<!--` as
        // comment-start and swallowed the entire rest of the document.
        let html = format!("<title><!--</title>{}", "<div>".repeat(100_000));
        // As above: assert the guard itself, not just the `None` outcome it
        // shares with the output-cap and conversion-failure paths.
        assert!(
            super::exceeds_safe_nesting_depth(&html),
            "expected the depth guard to trip on a rawtext + unterminated-comment prefix"
        );
        assert!(html_to_markdown(&html, BIG_CAP).is_none());
    }

    #[test]
    fn foreign_content_variants_do_not_bypass_the_depth_guard() {
        // The same shape as 6a for every name the reviewer confirmed stays
        // open in foreign content, under both `<svg>` and `<math>`.
        const N: usize = 1_000;
        const FOREIGN_STAYS_OPEN: &[&str] = &[
            "input", "area", "col", "link", "base", "param", "source", "track", "wbr",
        ];
        for wrapper in ["svg", "math"] {
            for name in FOREIGN_STAYS_OPEN {
                let html = format!("<{wrapper}>{}", format!("<{name}>").repeat(N));
                let label = format!("<{wrapper}> + <{name}>x{N}");
                let real_depth = real_dom_max_depth(&html);
                assert!(
                    real_depth > super::MAX_SCAN_DEPTH,
                    "{label}: expected real nesting past the cap, got {real_depth}                      -- this case would otherwise pass vacuously"
                );
                assert_guard_never_under_trips(&label, &html);
            }
        }
    }

    #[test]
    fn rawtext_variants_do_not_bypass_the_depth_guard() {
        // The same shape as 6b for every rawtext/RCDATA element.
        const N: usize = 1_000;
        for name in ["title", "textarea", "style", "script", "xmp"] {
            let html = format!("<{name}><!--</{name}>{}", "<div>".repeat(N));
            let label = format!("<{name}><!--</{name}> + <div>x{N}");
            let real_depth = real_dom_max_depth(&html);
            assert!(
                real_depth > super::MAX_SCAN_DEPTH,
                "{label}: expected real nesting past the cap, got {real_depth}                  -- this case would otherwise pass vacuously"
            );
            assert_guard_never_under_trips(&label, &html);
        }
    }

    #[test]
    fn depth_convention_is_the_real_dom_depth_and_the_cap_boundary_is_exact() {
        // The off-by-two this replaces: the old guard counted only
        // explicitly written elements, while the real DOM adds the implied
        // `html`/`body`, so `real > 512 => guard trips` was literally false
        // at real depths 513-514. Now guard depth *is* real depth.
        let at_cap = nested_divs(super::MAX_SCAN_DEPTH - 2);
        assert_eq!(
            real_dom_max_depth(&at_cap),
            super::MAX_SCAN_DEPTH,
            "sanity: N explicit divs must produce real depth N + 2"
        );
        let measured = super::measure_nesting_depth(&at_cap);
        assert_eq!(measured.depth, Some(super::MAX_SCAN_DEPTH));
        assert!(!measured.exceeds, "exactly at the cap must not trip");
        assert!(!measured.saw_unexpected_shape);
        assert!(html_to_markdown(&at_cap, BIG_CAP).is_some());

        let one_past = nested_divs(super::MAX_SCAN_DEPTH - 1);
        assert_eq!(real_dom_max_depth(&one_past), super::MAX_SCAN_DEPTH + 1);
        assert!(
            super::exceeds_safe_nesting_depth(&one_past),
            "one past the cap must trip"
        );
        assert!(html_to_markdown(&one_past, BIG_CAP).is_none());
    }

    #[test]
    fn parentless_sibling_shape_trips_the_guard() {
        // Fix for the parentless-sibling branch of `append_before_sibling`:
        // it used to only record `saw_unexpected_shape` and return, which
        // left the passed-in node (and anything later attached under it)
        // unreachable from the root -- an under-trip. This drives the
        // sink's `TreeSink::append_before_sibling` directly (rather than
        // hunting for real HTML that makes html5ever's tree builder call it
        // in this shape, which the differential/generative tests below
        // confirm never happens in practice) and asserts both flags trip.
        use html5ever::interface::tree_builder::{NodeOrText, TreeSink};
        use html5ever::{LocalName, Namespace, QualName};

        let tree = std::rc::Rc::new(super::DepthTree::new());
        let sink = super::DepthSink {
            tree: std::rc::Rc::clone(&tree),
        };
        let div_name = || {
            QualName::new(
                None,
                Namespace::from("http://www.w3.org/1999/xhtml"),
                LocalName::from("div"),
            )
        };
        // A node that exists in the arena but was never attached to any
        // parent -- the shape `append_before_sibling` cannot honour.
        let orphan_sibling = tree.push_node(Some(div_name()), false);
        let new_node = tree.push_node(Some(div_name()), false);

        assert!(!tree.exceeded.get(), "sanity: nothing has tripped yet");
        sink.append_before_sibling(&orphan_sibling, NodeOrText::AppendNode(new_node));
        assert!(
            tree.saw_unexpected_shape.get(),
            "the parentless-sibling shape must still be recorded"
        );
        assert!(
            tree.exceeded.get(),
            "the parentless-sibling shape must fail closed and trip the guard"
        );
    }

    // --- Differential test against the real parser -------------------------
    //
    // Five bypasses have now slipped through this guard's various
    // hand-rolled scanning approaches, each only found by a reviewer
    // hand-compiling a harness against the real parser. This brings that
    // harness into the suite permanently: for an adversarial corpus, assert
    // that whenever the REAL DOM's max depth exceeds the cap, the guard
    // also trips. The guard may over-trip (reject a document the real
    // parser would have handled); it must never under-trip.

    use html5ever::tendril::TendrilSink;
    use html5ever::{ParseOpts, parse_document};
    use markup5ever_rcdom::{Handle, NodeData, RcDom};

    /// The real maximum element-nesting depth of `html`, as built by the
    /// same parser (`html5ever`/`markup5ever_rcdom`) that `htmd` uses
    /// internally. Walked iteratively (an explicit `Vec` stack, never Rust
    /// call-stack recursion) so that measuring an adversarial, very-deep
    /// corpus item in a test never itself risks the exact failure mode this
    /// guard exists to prevent.
    fn real_dom_max_depth(html: &str) -> usize {
        let dom = parse_document(RcDom::default(), ParseOpts::default())
            .from_utf8()
            .read_from(&mut html.as_bytes())
            .expect("html5ever's TendrilSink does not fail on arbitrary UTF-8 input");
        let mut max_depth = 0usize;
        let mut stack: Vec<(Handle, usize)> = vec![(dom.document.clone(), 0)];
        while let Some((node, depth)) = stack.pop() {
            if matches!(node.data, NodeData::Element { .. }) {
                max_depth = max_depth.max(depth);
            }
            for child in node.children.borrow().iter() {
                stack.push((child.clone(), depth + 1));
            }
        }
        max_depth
    }

    /// Assert the guard's core invariant for one document: if the real DOM
    /// exceeds the cap, the guard must trip. Never asserts the converse.
    ///
    /// Two further checks ride along, both consequences of the guard now
    /// measuring with the real tree builder rather than approximating it:
    /// the measured depth must never come out *below* the real depth (the
    /// only documented divergence, `<template>` contents, over-counts), and
    /// the conservative parentless-sibling fallback must never fire.
    fn assert_guard_never_under_trips(label: &str, html: &str) {
        let real_depth = real_dom_max_depth(html);
        let measured = super::measure_nesting_depth(html);
        if real_depth > super::MAX_SCAN_DEPTH {
            assert!(
                measured.exceeds,
                "{label:?}: real DOM depth {real_depth} exceeds MAX_SCAN_DEPTH \
                 ({}) but the guard did not trip -- this is a bypass",
                super::MAX_SCAN_DEPTH
            );
        }
        if let Some(depth) = measured.depth {
            assert!(
                depth >= real_depth,
                "{label:?}: guard measured depth {depth} below the real DOM's \
                 {real_depth} -- the measurement has drifted from the parser"
            );
        }
        assert!(
            !measured.saw_unexpected_shape,
            "{label:?}: the parentless-sibling fallback fired; the sink is \
             no longer mirroring what html5ever asks of a tree sink"
        );
    }

    #[test]
    fn differential_guard_never_under_trips_against_the_real_parser() {
        // N=1000 keeps this fast (the dedicated 100_000-repetition cases
        // above remain as the slower crash regressions); it's already well
        // past MAX_SCAN_DEPTH (512) for every case here that should trip.
        const N: usize = 1_000;
        let corpus: Vec<(&str, String)> = vec![
            ("well_formed_div_opens", "<div>".repeat(N)),
            ("mismatched_close_padding", "<div></span>".repeat(N)),
            ("self_closing_non_void", "<div/>".repeat(N)),
            ("truncated_void_underscore", "<img_x>".repeat(N)),
            ("truncated_void_dot", "<br.x>".repeat(N)),
            ("truncated_void_at", "<hr@x>".repeat(N)),
            ("truncated_void_dot_input", "<input.x>".repeat(N)),
            ("truncated_void_non_ascii", "<img\u{00ef}>".repeat(N)),
            (
                "truncated_close_direction",
                format!("{}{}", "<div>".repeat(N), "</div_x>".repeat(N)),
            ),
            ("quoted_gt_in_attribute", "<div title=\"a>b\">".repeat(N)),
            (
                "comment_wrapped_close_padding",
                "<div><!--</div>-->".repeat(N),
            ),
            (
                "cdata_wrapped_close_padding",
                "<div><![CDATA[</div>]]>".repeat(N),
            ),
            (
                "processing_instruction_close_padding",
                "<div><?</div>></div>".repeat(N),
            ),
            (
                "bogus_comment_close_padding",
                "<div><!</div>></div>".repeat(N),
            ),
            (
                "doctype_wrapped_close_padding",
                "<div><!DOCTYPE </div>></div>".repeat(N),
            ),
            (
                "nul_padded_close_direction",
                format!("{}{}", "<div>".repeat(N), "</div\u{0}>".repeat(N)),
            ),
        ];

        for (label, html) in &corpus {
            assert_guard_never_under_trips(label, html);
        }

        // The comment-wrapped fake-tags case only makes sense as a shallow
        // *negative* check: real depth here is tiny (the fake tags never
        // leave the comment), so the invariant above would pass vacuously.
        // Assert the shallow direction explicitly instead of leaving it
        // untested.
        let shallow_comment_html = "<!-- <div><div><div> -->".repeat(N);
        let shallow_real_depth = real_dom_max_depth(&shallow_comment_html);
        assert!(
            shallow_real_depth <= 3,
            "sanity: fake tags inside a comment must not create real nesting, got depth {shallow_real_depth}"
        );
    }

    #[test]
    fn differential_guard_does_not_falsely_trip_on_a_shallow_legitimate_document() {
        // The flip side of the invariant above, checked directly for one
        // concrete legitimate case rather than corpus-wide: a real,
        // ~100-deep document must convert normally, not fall back to
        // plain text.
        let html = nested_divs(100);
        assert!(
            real_dom_max_depth(&html) < super::MAX_SCAN_DEPTH,
            "sanity: this corpus item's real depth must stay under the cap"
        );
        assert!(!super::exceeds_safe_nesting_depth(&html));
        assert!(html_to_markdown(&html, BIG_CAP).is_some());
    }

    // --- Generative (seeded PRNG) differential test -------------------------
    //
    // The hand-picked corpus above is backward-looking: every entry is a
    // known past bypass. This is a forward-looking discovery net: a small,
    // deterministic (fixed-seed) PRNG builds a few hundred randomized
    // documents from a token alphabet covering the same feature space —
    // void/non-void opens (with attributes, odd trailing bytes, NUL),
    // matching/mismatching closes, comments/CDATA/bogus-comments/doctypes,
    // rawtext blocks, and quoted attributes containing `>`/`</div>` — and
    // checks the same never-under-trips invariant against each one.

    /// Minimal deterministic PRNG (a Linear Congruential Generator, same
    /// constants as Knuth's MMIX/PCG family) -- no new dependency, and a
    /// fixed seed keeps this test's corpus, and thus CI, stable run to run.
    struct Lcg(u64);

    impl Lcg {
        fn next_u64(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0
        }

        /// Uniform-ish value in `0..bound` (`bound` must be nonzero).
        ///
        /// Takes the HIGH 32 bits and maps them onto the range by a
        /// widening multiply. An LCG's LOW bits have tiny periods -- bit 0
        /// simply alternates -- so the previous `next_u64() % bound`
        /// degenerated to near-strict alternation for small bounds,
        /// especially `below(2)`, which silently drained the variety out of
        /// the generated corpus. `lcg_high_bits_are_not_visibly_periodic`
        /// pins this down.
        fn below(&mut self, bound: usize) -> usize {
            if bound == 0 {
                return 0;
            }
            let high = u128::from(self.next_u64() >> 32);
            let scaled = (high * u128::try_from(bound).unwrap_or(u128::MAX)) >> 32;
            usize::try_from(scaled).unwrap_or(0)
        }

        fn choose<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[self.below(items.len())]
        }
    }

    const FUZZ_VOID_NAMES: &[&str] = &["br", "img", "input", "hr", "area", "meta"];
    const FUZZ_NON_VOID_NAMES: &[&str] =
        &["div", "span", "p", "section", "article", "b", "i", "li"];
    const FUZZ_RAWTEXT_NAMES: &[&str] = &["script", "style", "textarea", "title", "xmp"];
    /// Names that stay open inside foreign content (SVG/MathML) because
    /// they are not on the breakout list -- the shape of bypass 6a.
    const FUZZ_FOREIGN_STAYS_OPEN: &[&str] = &["input", "area", "col", "link", "track", "wbr"];
    /// Names on the foreign-content breakout list, which do *not* stay open.
    const FUZZ_FOREIGN_BREAKOUT: &[&str] = &["br", "img", "hr", "embed", "meta"];
    /// Bytes html5ever's tokenizer treats as name *continuation* (never a
    /// terminator), so appending one mid-name changes the parsed identity
    /// rather than ending it -- exactly the shape bypass 3 exploited.
    const FUZZ_ODD_SUFFIXES: &[&str] = &["_x", ".y", "@z", "\u{00ef}"];

    /// Append one random "token" to `doc`.
    ///
    /// The alphabet deliberately spans every shape that has ever bypassed
    /// this guard, including round 6's two families (foreign content, and
    /// rawtext elements whose body opens an unterminated comment), plus
    /// unbalanced rawtext and foreign openers so the generator can also
    /// build shapes nobody has hand-written yet.
    fn push_random_token(doc: &mut String, rng: &mut Lcg) {
        match rng.below(29) {
            // Plain non-void open (~42%): the main depth-building token.
            0..=9 => {
                let name = rng.choose(FUZZ_NON_VOID_NAMES);
                doc.push('<');
                doc.push_str(name);
                if rng.below(2) == 0 {
                    doc.push_str(" title=\"a>b\"");
                }
                doc.push('>');
            }
            // Non-void open with an odd trailing byte before `>`: must
            // still count as an open (bypass 3/5 shape), never as the void
            // element it superficially resembles.
            10..=12 => {
                let name = rng.choose(FUZZ_VOID_NAMES);
                let suffix = rng.choose(FUZZ_ODD_SUFFIXES);
                doc.push('<');
                doc.push_str(name);
                doc.push_str(suffix);
                doc.push('>');
            }
            // Genuine void open: must never hold the stack open.
            13 => {
                let name = rng.choose(FUZZ_VOID_NAMES);
                doc.push('<');
                doc.push_str(name);
                doc.push('>');
            }
            // Matching close.
            14 => {
                let name = rng.choose(FUZZ_NON_VOID_NAMES);
                doc.push_str("</");
                doc.push_str(name);
                doc.push('>');
            }
            // Mismatching / truncated / NUL-padded close: must never pop an
            // unrelated genuinely open element.
            15 => {
                let name = rng.choose(FUZZ_NON_VOID_NAMES);
                let suffix = rng.choose(FUZZ_ODD_SUFFIXES);
                doc.push_str("</");
                doc.push_str(name);
                doc.push_str(suffix);
                doc.push('>');
            }
            // Comment wrapping a fake close (bypass 4 shape).
            16 => doc.push_str("<div><!--</div>-->"),
            // CDATA / processing instruction / doctype wrapping a fake
            // close.
            17 => match rng.below(3) {
                0 => doc.push_str("<div><![CDATA[</div>]]>"),
                1 => doc.push_str("<div><?</div>>"),
                _ => doc.push_str("<div><!DOCTYPE </div>>"),
            },
            // Balanced rawtext block containing fake markup: the body is
            // text, so it must contribute no depth at all.
            18 => {
                let name = rng.choose(FUZZ_RAWTEXT_NAMES);
                doc.push('<');
                doc.push_str(name);
                doc.push_str("><div></div></");
                doc.push_str(name);
                doc.push('>');
            }
            // Rawtext element whose body opens an unterminated comment
            // (bypass 6b): the tree builder is in RCDATA/rawtext state, so
            // `<!--` is text and the element closes normally -- everything
            // after it nests for real.
            19 => {
                let name = rng.choose(FUZZ_RAWTEXT_NAMES);
                doc.push('<');
                doc.push_str(name);
                doc.push_str("><!--</");
                doc.push_str(name);
                doc.push('>');
            }
            // Unbalanced rawtext open, with no close at all.
            20 => {
                let name = rng.choose(FUZZ_RAWTEXT_NAMES);
                doc.push('<');
                doc.push_str(name);
                doc.push('>');
            }
            // Balanced foreign-content block whose children stay open
            // (bypass 6a shape) -- HTML void status does not apply in the
            // SVG/MathML namespace.
            21 => {
                let wrapper = if rng.below(2) == 0 { "svg" } else { "math" };
                let count = 1 + rng.below(4);
                doc.push('<');
                doc.push_str(wrapper);
                doc.push('>');
                for _ in 0..count {
                    doc.push('<');
                    doc.push_str(rng.choose(FUZZ_FOREIGN_STAYS_OPEN));
                    doc.push('>');
                }
                doc.push_str("</");
                doc.push_str(wrapper);
                doc.push('>');
            }
            // Bare, never-closed foreign-content opener: everything after
            // it is parsed in the foreign namespace.
            22 => {
                let wrapper = if rng.below(2) == 0 { "svg" } else { "math" };
                doc.push('<');
                doc.push_str(wrapper);
                doc.push('>');
            }
            // Foreign content using breakout names and a self-closing
            // slash, both of which *do* terminate the element there -- the
            // negative control for the two cases above.
            23 => {
                doc.push_str("<svg><path/>");
                doc.push('<');
                doc.push_str(rng.choose(FUZZ_FOREIGN_BREAKOUT));
                doc.push_str("></svg>");
            }
            // Table insertion mode: a `<div>` appearing directly inside
            // `<table>` (before any row/cell) is *foster parented* --
            // relocated to just before the table in the table's own
            // parent's child list -- rather than appended where a naive
            // sink would put it. `append`/`append_before_sibling` here
            // just mirror whatever html5ever tells them, so this exercises
            // the path where html5ever's target parent/sibling diverges
            // from "wherever we were about to attach". Closing the table
            // immediately afterward returns insertion mode to normal, so
            // (unlike a bare unclosed `<table>`) this does not flatten
            // every token generated for the rest of the document.
            24 => doc.push_str("<table><div></table>"),
            // `<template>` open, with content, closed: exercises
            // `get_template_contents` (the contents node this sink attaches
            // as an ordinary child in `create_element`, unlike `RcDom`,
            // which keeps it off the child list -- the first documented
            // residual). Left *unclosed* this token would be far more
            // disruptive than that residual describes: while a template
            // stays open, `RcDom` routes everything generated afterward
            // into its content document fragment rather than the normal
            // tree, which makes it invisible to `real_dom_max_depth`'s
            // ordinary child walk -- silently deleting the rest of the
            // corpus item's depth from the reference oracle. Closing it
            // keeps the token's effect local to itself.
            25 => doc.push_str("<template><div></template>"),
            // Adoption-agency shape: a formatting element (`<b>`) left
            // open around a block element (`<p>`), closed out of the
            // naive nesting order, is the textbook trigger for the
            // adoption agency algorithm, which calls `reparent_children`
            // -- the second documented residual (a stale early-exit
            // estimate, corrected by the exact recomputation at parse
            // end). `<p>` stays open afterward, so later tokens keep
            // nesting normally rather than the corpus flattening.
            26 => doc.push_str("<b><p></b>"),
            // MathML annotation-xml integration point: an `<annotation-xml>`
            // with a `text/html`/`application/xhtml+xml` `encoding`
            // attribute is where `is_mathml_annotation_xml_integration_point`
            // must answer `true` so ordinary HTML content re-enters the
            // tree instead of staying in the MathML namespace.
            27 => doc.push_str("<math><annotation-xml encoding=\"text/html\">"),
            // SVG foreignObject integration point: also re-admits ordinary
            // HTML content inside foreign content, exercised via the same
            // hook from the SVG side.
            _ => doc.push_str("<svg><foreignObject>"),
        }
    }

    /// Build one deterministic pseudo-random document of roughly
    /// `token_count` tokens.
    fn generate_fuzz_doc(seed: u64, token_count: usize) -> String {
        let mut rng = Lcg(seed);
        let mut doc = String::new();
        for _ in 0..token_count {
            push_random_token(&mut doc, &mut rng);
        }
        doc
    }

    #[test]
    fn lcg_high_bits_are_not_visibly_periodic() {
        // `below` must use high LCG bits: low-bit `% bound` made `below(2)`
        // nearly alternate. Two cheap checks that a strictly (or
        // near-strictly) alternating sequence cannot pass: at least one run
        // of three equal values, and a transition count far from the 511 an
        // alternating sequence produces.
        let mut rng = Lcg(0x5EED_00F4_0000_0001);
        let bits: Vec<usize> = (0..512).map(|_| rng.below(2)).collect();
        let transitions = bits.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(
            (150..=360).contains(&transitions),
            "below(2) produced {transitions} transitions in 512 draws; \
             ~256 is random, 511 is strict alternation"
        );
        assert!(
            bits.windows(3).any(|w| w[0] == w[1] && w[1] == w[2]),
            "below(2) never produced a run of three -- still periodic"
        );
        // And the wider bound must actually cover its whole range (29 is
        // `push_random_token`'s current alphabet size).
        let mut rng = Lcg(0x5EED_00F4_0000_0002);
        let mut seen = [false; 29];
        for _ in 0..2_000 {
            seen[rng.below(29)] = true;
        }
        assert!(
            seen.iter().all(|hit| *hit),
            "below(29) left holes in its range"
        );
    }

    #[test]
    fn generative_fuzz_guard_never_under_trips_against_the_real_parser() {
        // Fixed seed base: deterministic corpus, stable CI.
        //
        // Reviewer finding (c): every generated document used to land at
        // real depth 837-992, so the 512 boundary was never probed and a
        // boundary bug would have been invisible. Document size now ramps
        // linearly across the corpus, spreading real depth from a few dozen
        // to well over a thousand, and the assertions below fail if that
        // spread ever stops straddling -- and closely approaching -- the
        // cap.
        const DOC_COUNT: usize = 200;
        const SEED_BASE: u64 = 0x5EED_00F4_0000_0001;
        /// Half-width of the "close to the cap" window that must be hit.
        const BOUNDARY_WINDOW: usize = 64;

        let mut below_cap = 0usize;
        let mut above_cap = 0usize;
        let mut near_boundary = 0usize;
        for i in 0..DOC_COUNT {
            // 25 .. 8_980 tokens: a linear ramp, so real depth sweeps the
            // whole interesting range instead of clustering above it.
            let token_count = 25 + i * 45;
            let seed = SEED_BASE.wrapping_add(
                u64::try_from(i)
                    .unwrap_or(0)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15),
            );
            let html = generate_fuzz_doc(seed, token_count);
            let real_depth = real_dom_max_depth(&html);
            if real_depth > super::MAX_SCAN_DEPTH {
                above_cap += 1;
            } else {
                below_cap += 1;
            }
            if real_depth.abs_diff(super::MAX_SCAN_DEPTH) <= BOUNDARY_WINDOW {
                near_boundary += 1;
            }
            assert_guard_never_under_trips(&format!("fuzz#{i}(tokens={token_count})"), &html);
        }

        assert!(
            above_cap > 0,
            "generative corpus never exceeded MAX_SCAN_DEPTH -- the invariant \
             above passed vacuously; widen the token alphabet's open-tag bias"
        );
        assert!(
            below_cap > 0,
            "generative corpus never stayed under MAX_SCAN_DEPTH -- the size \
             ramp starts too high to probe the boundary from below"
        );
        assert!(
            near_boundary > 0,
            "no generated document landed within {BOUNDARY_WINDOW} of the cap \
             -- the size distribution has drifted away from the boundary again"
        );
    }
}
