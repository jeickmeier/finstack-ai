use crate::JournalSearchSource;
use finstack_ai_search_core::{
    SearchError, SearchHit, SourceReferencePage, validate_catalog_request,
};
use rusqlite::types::Value;

impl JournalSearchSource {
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
            .map_err(|_| SearchError::invalid("journal_catalog_cursor"))?;
        if after < 0 {
            return Err(SearchError::invalid("journal_catalog_cursor"));
        }
        if self.config.sessions.is_empty() {
            return Ok(SourceReferencePage {
                references: vec![],
                next_cursor: None,
            });
        }
        let scope = self.scope_digest.clone();
        let sessions = self.config.sessions.clone();
        self.database.call(move |db| {
            let placeholders=std::iter::repeat_n("?",sessions.len()).collect::<Vec<_>>().join(",");
            let sql=format!("SELECT rowid,hit_json FROM entries WHERE scope=? AND rowid>? AND session IN ({placeholders}) ORDER BY rowid LIMIT ?");
            let mut args=vec![Value::Text(scope),Value::Integer(after)];
            args.extend(sessions.into_iter().map(|s|Value::Text(s.to_string())));
            args.push(Value::Integer(i64::try_from(limit+1).map_err(|_|SearchError::invalid("journal_catalog_limit"))?));
            let mut statement=db.prepare(&sql).map_err(|_|SearchError::SearchUnavailable)?;
            let rows=statement.query_map(rusqlite::params_from_iter(args),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).map_err(|_|SearchError::SearchUnavailable)?;
            let mut references=Vec::new();let mut last=after;let mut more=false;
            for row in rows {
                let (id,hit)=row.map_err(|_|SearchError::SearchUnavailable)?;
                if references.len()==limit {more=true;break;}
                let hit:SearchHit=serde_json::from_str(&hit).map_err(|_|SearchError::invalid("journal_catalog"))?;
                hit.reference.key()?;references.push(hit.reference);last=id;
            }
            Ok(SourceReferencePage {references,next_cursor:more.then(||last.to_string())})
        }).await
    }
}
