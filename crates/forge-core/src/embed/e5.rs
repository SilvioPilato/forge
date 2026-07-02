#[cfg(feature = "onnx")]
pub(crate) fn fetch_model_files(
    cache_dir: &std::path::Path,
    model_id: &str,
    show_progress: bool,
) -> Result<(std::path::PathBuf, std::path::PathBuf), super::EmbedError> {
    let cache = hf_hub::Cache::new(cache_dir.join("models"));
    let api = hf_hub::api::sync::ApiBuilder::from_cache(cache)
        .with_progress(show_progress)
        .build()
        .map_err(|e| super::EmbedError(format!("hf-hub api: {e}")))?;
    let repo = api.model(model_id.to_string());

    let onnx_path = repo
        .get("onnx/model.onnx")
        .map_err(|e| super::EmbedError(format!("download model.onnx: {e}")))?;
    let tokenizer_path = repo
        .get("tokenizer.json")
        .map_err(|e| super::EmbedError(format!("download tokenizer.json: {e}")))?;
    Ok((onnx_path, tokenizer_path))
}

#[cfg(feature = "onnx")]
pub struct E5Embedder {
    model_id: String,
    session: std::sync::Mutex<ort::session::Session>,
    tokenizer: tokenizers::Tokenizer,
}

#[cfg(feature = "onnx")]
impl E5Embedder {
    pub fn load(
        cache_dir: &std::path::Path,
        model_id: &str,
    ) -> Result<E5Embedder, super::EmbedError> {
        let (onnx_path, tokenizer_path) = fetch_model_files(cache_dir, model_id, false)?;

        let mut tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| super::EmbedError(format!("tokenizer load: {e}")))?;

        let mut padding = tokenizers::PaddingParams::default();
        padding.pad_id = 0;
        padding.pad_type_id = 0;
        padding.pad_token = "[PAD]".to_string();
        padding.strategy = tokenizers::PaddingStrategy::Fixed(512);
        tokenizer.with_padding(Some(padding));

        let mut truncation = tokenizers::TruncationParams::default();
        truncation.max_length = 512;
        truncation.strategy = tokenizers::TruncationStrategy::LongestFirst;
        truncation.direction = tokenizers::TruncationDirection::Right;
        let _ = tokenizer.with_truncation(Some(truncation));

        let session = ort::session::Session::builder()
            .map_err(|e| super::EmbedError(format!("session builder: {e}")))?
            .commit_from_file(onnx_path)
            .map_err(|e| super::EmbedError(format!("load model: {e}")))?;

        Ok(E5Embedder {
            model_id: model_id.to_string(),
            session: std::sync::Mutex::new(session),
            tokenizer,
        })
    }

    fn embed_batch(
        &self,
        texts: &[String],
        prefix: &str,
    ) -> Result<Vec<super::Vector>, super::EmbedError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let mut all_embeddings: Vec<super::Vector> = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(16) {
            let prefixed: Vec<String> = chunk.iter().map(|t| format!("{prefix}{t}")).collect();

            let encodings = self
                .tokenizer
                .encode_batch(prefixed, false)
                .map_err(|e| super::EmbedError(format!("tokenize: {e}")))?;

            let batch_size = encodings.len();
            let seq_len = match encodings.first() {
                Some(e) => e.get_ids().len(),
                None => continue,
            };

            let total = batch_size * seq_len;
            let mut flat_ids: Vec<i64> = Vec::with_capacity(total);
            let mut flat_mask: Vec<i64> = Vec::with_capacity(total);
            let mut flat_type_ids: Vec<i64> = Vec::with_capacity(total);

            for enc in &encodings {
                flat_ids.extend(enc.get_ids().iter().map(|&id| id as i64));
                flat_mask.extend(enc.get_attention_mask().iter().map(|&m| m as i64));
                flat_type_ids.extend(enc.get_type_ids().iter().map(|&t| t as i64));
            }

            let shape = vec![batch_size as i64, seq_len as i64];

            // Retain mask data for mean pooling after tensors consume the Vecs
            let mask_data = flat_mask.clone();

            let input_ids =
                ort::value::Tensor::from_array((shape.clone(), flat_ids.into_boxed_slice()))
                    .map_err(|e| super::EmbedError(format!("create input_ids: {e}")))?;
            let attention_mask =
                ort::value::Tensor::from_array((shape.clone(), flat_mask.into_boxed_slice()))
                    .map_err(|e| super::EmbedError(format!("create attention_mask: {e}")))?;
            let token_type_ids =
                ort::value::Tensor::from_array((shape.clone(), flat_type_ids.into_boxed_slice()))
                    .map_err(|e| super::EmbedError(format!("create token_type_ids: {e}")))?;

            let mut session = self.session.lock().unwrap();
            let outputs = session
                .run(ort::inputs![
                    "input_ids" => input_ids,
                    "attention_mask" => attention_mask,
                    "token_type_ids" => token_type_ids,
                ])
                .map_err(|e| super::EmbedError(format!("inference: {e}")))?;

            let (_shape, hidden_data): (_, &[f32]) = outputs["last_hidden_state"]
                .try_extract_tensor()
                .map_err(|e| super::EmbedError(format!("extract last_hidden_state: {e}")))?;

            let hidden_size = hidden_data.len() / (batch_size * seq_len);
            let mut chunk_embeddings =
                Self::mean_pool(hidden_data, &mask_data, batch_size, seq_len, hidden_size);

            for emb in &mut chunk_embeddings {
                Self::l2_normalize(emb);
            }

            all_embeddings.extend(chunk_embeddings);
        }

        Ok(all_embeddings)
    }

    fn mean_pool(
        hidden: &[f32],
        attention_mask: &[i64],
        batch_size: usize,
        seq_len: usize,
        hidden_size: usize,
    ) -> Vec<super::Vector> {
        let mut result = vec![vec![0.0f32; hidden_size]; batch_size];
        for b in 0..batch_size {
            let offset = b * seq_len * hidden_size;
            let mask_offset = b * seq_len;
            let mut mask_sum = 0.0f32;
            for s in 0..seq_len {
                let m = attention_mask[mask_offset + s] as f32;
                if m > 0.0 {
                    mask_sum += m;
                    for h in 0..hidden_size {
                        result[b][h] += hidden[offset + s * hidden_size + h] * m;
                    }
                }
            }
            if mask_sum > 0.0 {
                for h in 0..hidden_size {
                    result[b][h] /= mask_sum;
                }
            }
        }
        result
    }

    fn l2_normalize(v: &mut [f32]) {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    }
}

#[cfg(feature = "onnx")]
impl super::Embedder for E5Embedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed_passages(&self, texts: &[String]) -> Result<Vec<super::Vector>, super::EmbedError> {
        self.embed_batch(texts, "passage: ")
    }

    fn embed_query(&self, text: &str) -> Result<super::Vector, super::EmbedError> {
        let results = self.embed_batch(&[text.to_string()], "query: ")?;
        Ok(results.into_iter().next().unwrap_or_default())
    }
}

#[cfg(test)]
#[cfg(feature = "onnx")]
mod tests {
    use super::super::Embedder;
    use super::*;

    #[test]
    #[ignore = "downloads ~120MB model on first run"]
    fn e5_embeds_and_multilingual_neighbors_rank() {
        let cache_dir = std::env::temp_dir().join("forge-onnx-test-cache");
        let _ = std::fs::create_dir_all(&cache_dir);
        let e = E5Embedder::load(&cache_dir, "intfloat/multilingual-e5-small").unwrap();
        let v = e.embed_passages(&["passage text".into()]).unwrap();
        assert_eq!(v[0].len(), 384);

        // cross-lingual: Italian > English irrelevant
        let q = e.embed_query("the database is slow").unwrap();
        let p1 = e.embed_passages(&["il database è lento".into()]).unwrap();
        let p2 = e.embed_passages(&["the sky is blue".into()]).unwrap();
        let sim1 = super::super::cosine(&q, &p1[0]);
        let sim2 = super::super::cosine(&q, &p2[0]);
        assert!(
            sim1 > sim2,
            "cross-lingual match should rank higher: {sim1} vs {sim2}"
        );
    }
}
