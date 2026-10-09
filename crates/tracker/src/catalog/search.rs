//! The catalog's search index (D-130): listed bundles' text, in a Tantivy index kept in memory.
//!
//! The index mirrors the store and is rebuilt from it when the catalog opens; [`super::Catalog`]
//! updates it whenever a listing is listed, changed or unpublished. It finds the listings that
//! match a search, scores how well, and counts the matches' categories and tags. Ranking by
//! popularity is [`super::rank`]'s.
//!
//! Matching forgives typos: each word of the search matches words within an edit distance that
//! grows with its length, and the last word also matches as a prefix, so results follow typing.
//! Every word must match somewhere. Exact matches score by BM25 and outrank fuzzy ones, and
//! matches in titles outrank matches in descriptions.

use std::collections::HashSet;

use tantivy::collector::{Collector, FacetCollector, SegmentCollector};
use tantivy::columnar::{BytesColumn, Column};
use tantivy::query::{
    AllQuery, BooleanQuery, BoostQuery, ConstScoreQuery, FuzzyTermQuery, Occur, Query, TermQuery,
};
use tantivy::schema::{
    BytesOptions, FAST, FacetOptions, Field, IndexRecordOption, Schema, TextFieldIndexing,
    TextOptions,
};
use tantivy::tokenizer::{
    AsciiFoldingFilter, LowerCaser, RemoveLongFilter, SimpleTokenizer, TextAnalyzer,
};
use tantivy::{
    DocId, Index, IndexReader, IndexWriter, ReloadPolicy, Score, SegmentReader, TantivyDocument,
    Term,
};
use tempest_protocol::{BundleId, Hash};
use tempest_tracker_api::listing::{FacetCount, Facets, MAX_TAG_FACETS};

use super::store::Indexed;

/// The tokenizer of every text field: words of letters and digits, lowercased and folded to
/// ASCII, so `Café` matches `cafe`.
const TOKENIZER: &str = "catalog";

/// The longest word indexed or searched for.
const MAX_WORD: usize = 40;

/// The most words of a search that count; more add little and cost much.
const MAX_WORDS: usize = 8;

/// How much a fuzzy match scores, against a field's weight; exact matches add BM25 on top.
const FUZZY_SCORE: Score = 0.5;

/// The index writer's memory, Tantivy's least.
const WRITER_MEMORY: usize = 15_000_000;

/// The fields searched, with how much a match in each weighs.
#[derive(Debug, Clone, Copy)]
struct Fields {
    bundle: Field,
    created: Field,
    category: Field,
    tags: Field,
    title: Field,
    description: Field,
    tag_words: Field,
    content: Field,
    category_name: Field,
}

impl Fields {
    fn weighted(&self) -> [(Field, Score); 5] {
        [
            (self.title, 3.0),
            (self.tag_words, 2.0),
            (self.content, 2.0),
            (self.category_name, 1.0),
            (self.description, 1.0),
        ]
    }
}

/// A listing that matched a search.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// The listed bundle.
    pub bundle: BundleId,
    /// When its listing was first asked for, for ranking newest first.
    pub created: u64,
    /// How well it matched the search's text; the same for every listing without text.
    pub score: Score,
}

/// A search's matches, in no order, and how they spread over categories and tags.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// Every match.
    pub matches: Vec<Match>,
    /// The categories' and tags' counts.
    pub facets: Facets,
}

/// The catalog's search index.
pub struct SearchIndex {
    index: Index,
    reader: IndexReader,
    writer: IndexWriter,
    fields: Fields,
}

impl std::fmt::Debug for SearchIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchIndex").finish_non_exhaustive()
    }
}

impl SearchIndex {
    /// An empty index.
    pub fn new() -> tantivy::Result<Self> {
        let mut schema = Schema::builder();
        let text = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );
        let fields = Fields {
            bundle: schema
                .add_bytes_field("bundle", BytesOptions::default().set_indexed().set_fast()),
            created: schema.add_u64_field("created", FAST),
            category: schema.add_facet_field("category", FacetOptions::default()),
            tags: schema.add_facet_field("tags", FacetOptions::default()),
            title: schema.add_text_field("title", text.clone()),
            description: schema.add_text_field("description", text.clone()),
            tag_words: schema.add_text_field("tag_words", text.clone()),
            content: schema.add_text_field("content", text.clone()),
            category_name: schema.add_text_field("category_name", text),
        };
        let index = Index::create_in_ram(schema.build());
        index.tokenizers().register(TOKENIZER, analyzer());
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        let writer = index.writer_with_num_threads(1, WRITER_MEMORY)?;
        Ok(Self {
            index,
            reader,
            writer,
            fields,
        })
    }

    /// Replaces everything in the index with `listings`, each with its category's name.
    pub fn rebuild<'a>(
        &mut self,
        listings: impl IntoIterator<Item = (&'a Indexed, Option<&'a str>)>,
    ) -> tantivy::Result<()> {
        self.writer.delete_all_documents()?;
        for (listing, category_name) in listings {
            self.writer
                .add_document(self.document(listing, category_name))?;
        }
        self.commit()
    }

    /// Adds `listing`, with its category's name, replacing what the index had of its bundle.
    pub fn put(&mut self, listing: &Indexed, category_name: Option<&str>) -> tantivy::Result<()> {
        self.writer.delete_term(self.bundle_term(&listing.bundle));
        self.writer
            .add_document(self.document(listing, category_name))?;
        self.commit()
    }

    /// Removes `bundle` from the index, if it is there.
    pub fn remove(&mut self, bundle: &BundleId) -> tantivy::Result<()> {
        self.writer.delete_term(self.bundle_term(bundle));
        self.commit()
    }

    fn commit(&mut self) -> tantivy::Result<()> {
        self.writer.commit()?;
        self.reader.reload()
    }

    fn bundle_term(&self, bundle: &BundleId) -> Term {
        Term::from_field_bytes(self.fields.bundle, bundle.as_bytes())
    }

    fn document(&self, listing: &Indexed, category_name: Option<&str>) -> TantivyDocument {
        let f = &self.fields;
        let mut doc = TantivyDocument::new();
        doc.add_bytes(f.bundle, listing.bundle.as_bytes());
        doc.add_u64(f.created, listing.created);
        doc.add_text(f.title, &listing.fields.title);
        doc.add_text(f.description, &listing.fields.description);
        doc.add_text(f.content, &listing.content_name);
        if let Some(category) = &listing.fields.category {
            doc.add_facet(f.category, facet(category));
            doc.add_text(f.category_name, category_name.unwrap_or(category));
        }
        for tag in &listing.fields.tags {
            doc.add_facet(f.tags, facet(tag));
            doc.add_text(f.tag_words, tag);
        }
        doc
    }

    /// The listings matching `text`, in `category` if one is given, with every one of `tags`.
    pub fn search(
        &self,
        text: &str,
        category: Option<&str>,
        tags: &[String],
    ) -> tantivy::Result<Found> {
        let searcher = self.reader.searcher();
        let text = self.text_query(text);
        let tag_filters = || -> Vec<Box<dyn Query>> {
            tags.iter()
                .map(|tag| filter(self.fields.tags, tag))
                .collect()
        };

        // Category counts ignore the category asked for.
        let mut by_category = FacetCollector::for_field("category");
        by_category.add_facet("/");
        let without_category = all_of(text.box_clone(), tag_filters());
        let category_counts = searcher.search(&without_category, &by_category)?;

        let mut filters = tag_filters();
        if let Some(category) = category {
            filters.push(filter(self.fields.category, category));
        }
        let mut by_tag = FacetCollector::for_field("tags");
        by_tag.add_facet("/");
        let query = all_of(text, filters);
        let (matches, tag_counts) = searcher.search(&query, &(Matches, by_tag))?;

        let counts = |counts: &tantivy::collector::FacetCounts, most: usize| {
            let mut counted: Vec<FacetCount> = counts
                .get("/")
                .filter_map(|(facet, count)| {
                    let value = facet.to_path().last()?.to_string();
                    Some(FacetCount { value, count })
                })
                .collect();
            counted.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
            counted.truncate(most);
            counted
        };
        Ok(Found {
            matches,
            facets: Facets {
                categories: counts(&category_counts, usize::MAX),
                tags: counts(&tag_counts, MAX_TAG_FACETS),
            },
        })
    }

    /// The query for search text: every word must match a field, exactly or nearly.
    fn text_query(&self, text: &str) -> Box<dyn Query> {
        let words = self.words(text);
        if words.is_empty() {
            return Box::new(AllQuery);
        }
        let last = words.len() - 1;
        let clauses = words
            .iter()
            .enumerate()
            .map(|(i, word)| {
                let mut either: Vec<Box<dyn Query>> = Vec::new();
                for (field, weight) in self.fields.weighted() {
                    let term = Term::from_field_text(field, word);
                    let distance = typos_allowed(word);
                    let fuzzy = match i == last {
                        true => FuzzyTermQuery::new_prefix(term.clone(), distance, true),
                        false => FuzzyTermQuery::new(term.clone(), distance, true),
                    };
                    let exact = TermQuery::new(term, IndexRecordOption::WithFreqs);
                    either.push(Box::new(BoostQuery::new(Box::new(exact), weight)));
                    either.push(Box::new(BoostQuery::new(
                        Box::new(fuzzy),
                        weight * FUZZY_SCORE,
                    )));
                }
                Box::new(BooleanQuery::union(either)) as Box<dyn Query>
            })
            .collect();
        Box::new(BooleanQuery::intersection(clauses))
    }

    /// The distinct words of `text` as the index has them, at most [`MAX_WORDS`].
    fn words(&self, text: &str) -> Vec<String> {
        let mut analyzer = self
            .index
            .tokenizers()
            .get(TOKENIZER)
            .unwrap_or_else(analyzer);
        let mut stream = analyzer.token_stream(text);
        let mut seen = HashSet::new();
        let mut words = Vec::new();
        while words.len() < MAX_WORDS && stream.advance() {
            let word = stream.token().text.clone();
            if seen.insert(word.clone()) {
                words.push(word);
            }
        }
        words
    }
}

fn analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(MAX_WORD))
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .build()
}

/// How many typos a search word may have: none in short words, where one typo makes another
/// word, then one, and two from seven letters on.
fn typos_allowed(word: &str) -> u8 {
    match word.chars().count() {
        0..=3 => 0,
        4..=6 => 1,
        _ => 2,
    }
}

/// The facet for a category's slug or a tag: one level below the root.
fn facet(value: &str) -> tantivy::schema::Facet {
    tantivy::schema::Facet::from_path([value])
}

/// A filter that a listing must pass and that adds nothing to its score.
fn filter(field: Field, value: &str) -> Box<dyn Query> {
    let term = Term::from_facet(field, &facet(value));
    let query = TermQuery::new(term, IndexRecordOption::Basic);
    Box::new(ConstScoreQuery::new(Box::new(query), 0.0))
}

/// `query` limited to what passes every one of `filters`.
fn all_of(query: Box<dyn Query>, filters: Vec<Box<dyn Query>>) -> Box<dyn Query> {
    if filters.is_empty() {
        return query;
    }
    let mut clauses = vec![(Occur::Must, query)];
    clauses.extend(filters.into_iter().map(|filter| (Occur::Must, filter)));
    Box::new(BooleanQuery::new(clauses))
}

/// Collects every match with its bundle, creation time and score.
struct Matches;

struct SegmentMatches {
    bundles: Option<BytesColumn>,
    created: Column<u64>,
    matches: Vec<Match>,
    buffer: Vec<u8>,
}

impl Collector for Matches {
    type Fruit = Vec<Match>;
    type Child = SegmentMatches;

    fn for_segment(&self, _: u32, segment: &SegmentReader) -> tantivy::Result<SegmentMatches> {
        let fast = segment.fast_fields();
        Ok(SegmentMatches {
            bundles: fast.bytes("bundle")?,
            created: fast.u64("created")?,
            matches: Vec::new(),
            buffer: Vec::new(),
        })
    }

    fn requires_scoring(&self) -> bool {
        true
    }

    fn merge_fruits(&self, fruits: Vec<Vec<Match>>) -> tantivy::Result<Vec<Match>> {
        Ok(fruits.into_iter().flatten().collect())
    }
}

impl SegmentCollector for SegmentMatches {
    type Fruit = Vec<Match>;

    fn collect(&mut self, doc: DocId, score: Score) {
        let Some(bundles) = &self.bundles else {
            return;
        };
        let Some(ord) = bundles.term_ords(doc).next() else {
            return;
        };
        self.buffer.clear();
        if !matches!(bundles.ord_to_bytes(ord, &mut self.buffer), Ok(true)) {
            return;
        }
        let Ok(bundle) = <[u8; 32]>::try_from(self.buffer.as_slice()) else {
            return;
        };
        self.matches.push(Match {
            bundle: Hash::new(bundle),
            created: self.created.first(doc).unwrap_or(0),
            score,
        });
    }

    fn harvest(self) -> Vec<Match> {
        self.matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::store::ListingFields;

    fn listing(n: u8, title: &str, description: &str, category: &str, tags: &[&str]) -> Indexed {
        Indexed {
            bundle: Hash::new([n; 32]),
            fields: ListingFields {
                title: title.into(),
                description: description.into(),
                category: Some(category.into()).filter(|c: &String| !c.is_empty()),
                tags: tags.iter().map(|t| t.to_string()).collect(),
            },
            content_name: format!("content-{n}"),
            created: u64::from(n),
        }
    }

    fn index(listings: &[Indexed]) -> SearchIndex {
        let mut index = SearchIndex::new().unwrap();
        index.rebuild(listings.iter().map(|l| (l, None))).unwrap();
        index
    }

    fn found(index: &SearchIndex, text: &str) -> Vec<u8> {
        let mut matches = index.search(text, None, &[]).unwrap().matches;
        matches.sort_by(|a, b| b.score.total_cmp(&a.score));
        matches.iter().map(|m| m.bundle.as_bytes()[0]).collect()
    }

    fn sample() -> Vec<Indexed> {
        vec![
            listing(
                1,
                "Global weather archive",
                "Hourly readings",
                "datasets",
                &["climate", "weather"],
            ),
            listing(
                2,
                "Station notes",
                "Notes about the weather station",
                "documents",
                &["climate"],
            ),
            listing(
                3,
                "Llama weights",
                "A language model",
                "models",
                &["machine-learning"],
            ),
            listing(4, "Café menu", "", "", &[]),
        ]
    }

    #[test]
    fn typos_and_prefixes_still_match() {
        let index = index(&sample());
        assert_eq!(found(&index, "wether"), [1, 2]);
        assert_eq!(found(&index, "weathr archve"), [1]);
        assert_eq!(found(&index, "lla"), [3]);
        assert_eq!(found(&index, "cafe"), [4]);
        assert_eq!(found(&index, "machine learning"), [3]);
        assert_eq!(found(&index, "content-3"), [3]);
        assert!(found(&index, "zebra").is_empty());
    }

    #[test]
    fn titles_outrank_descriptions() {
        let index = index(&sample());
        assert_eq!(found(&index, "weather"), [1, 2]);
        assert_eq!(found(&index, "notes"), [2]);
    }

    #[test]
    fn empty_text_matches_everything() {
        let index = index(&sample());
        assert_eq!(index.search("  ", None, &[]).unwrap().matches.len(), 4);
        assert_eq!(index.search("!!", None, &[]).unwrap().matches.len(), 4);
    }

    #[test]
    fn filters_and_facets() {
        let index = index(&sample());
        let climate = ["climate".to_string()];
        let found = index.search("", Some("datasets"), &climate).unwrap();
        assert_eq!(found.matches.len(), 1);
        let counts = |counts: &[FacetCount]| -> Vec<(String, u64)> {
            counts.iter().map(|c| (c.value.clone(), c.count)).collect()
        };
        // Categories count as if no category were picked; tags count the matches.
        assert_eq!(
            counts(&found.facets.categories),
            [("datasets".into(), 1), ("documents".into(), 1)]
        );
        assert_eq!(
            counts(&found.facets.tags),
            [("climate".into(), 1), ("weather".into(), 1)]
        );
    }

    #[test]
    fn puts_replace_and_removes_remove() {
        let mut listings = sample();
        let mut index = index(&listings);
        listings[0].fields.title = "Sea levels".into();
        index.put(&listings[0], None).unwrap();
        assert_eq!(found(&index, "sea"), [1]);
        assert_eq!(found(&index, "archive"), Vec::<u8>::new());
        assert_eq!(index.search("", None, &[]).unwrap().matches.len(), 4);
        index.remove(&listings[0].bundle).unwrap();
        assert!(found(&index, "sea").is_empty());
        let rest = index.search("", None, &[]).unwrap().matches;
        assert_eq!(rest.len(), 3);
        assert!(
            rest.iter()
                .all(|m| m.created == u64::from(m.bundle.as_bytes()[0]))
        );
    }
}
