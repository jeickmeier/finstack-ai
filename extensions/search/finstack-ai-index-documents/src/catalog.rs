use crate::{DocumentInput, DocumentSearchSource};
use finstack_ai_search_core::{
    SearchError, SourceRef, SourceReferencePage, validate_catalog_request,
};
use rusqlite::params;

impl DocumentSearchSource {
    pub(crate) async fn catalog(
        &self,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<SourceReferencePage, SearchError> {
        validate_catalog_request(cursor.as_deref(), limit)?;
        let after = cursor
            .as_deref()
            .unwrap_or("0")
            .parse::<i64>()
            .map_err(|_| SearchError::invalid("document_catalog_cursor"))?;
        if after < 0 {
            return Err(SearchError::invalid("document_catalog_cursor"));
        }
        let scope = self.scope_digest.clone();
        let digest = self.chunker_digest;
        self.database.call(move |db| {
            let mut statement=db.prepare("SELECT c.rowid,c.ordinal,d.input_json FROM chunks c JOIN documents d ON d.document_key=c.document_key WHERE d.scope_digest=?1 AND d.chunker_digest=?2 AND c.rowid>?3 ORDER BY c.rowid LIMIT ?4").map_err(|_|SearchError::SearchUnavailable)?;
            let rows=statement.query_map(params![scope,digest.to_hex(),after,limit+1],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,u32>(1)?,r.get::<_,String>(2)?))).map_err(|_|SearchError::SearchUnavailable)?;
            let mut references=Vec::new();let mut last=after;let mut more=false;
            for row in rows {
                let (id,ordinal,input)=row.map_err(|_|SearchError::SearchUnavailable)?;
                if references.len()==limit {more=true;break;}
                let input:DocumentInput=serde_json::from_str(&input).map_err(|_|SearchError::invalid("document_catalog"))?;
                let reference=SourceRef::ArtifactChunk {artifact:Box::new(input.artifact),ordinal,chunker:digest};reference.key()?;
                references.push(reference);last=id;
            }
            Ok(SourceReferencePage {references,next_cursor:more.then(||last.to_string())})
        }).await
    }
}
