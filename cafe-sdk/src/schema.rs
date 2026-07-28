use cafe_types::{keys, Chunk, EvaluatorSchema, SessionConfig};

/// Publish an evaluator's schema to the `__schema__` session.
///
/// Called once by each evaluator on startup after connecting to the bus.
/// If the `__schema__` session already exists this is a no-op on the
/// create side; the schema chunk is published regardless so a reconnecting
/// evaluator re-advertises its schema.
pub async fn announce_schema(
    bus: &crate::bus::BusClient,
    schema: EvaluatorSchema,
) -> Result<(), crate::SdkError> {
    // Create session (idempotent — ignore "already exists" errors)
    match bus
        .create_session(
            cafe_types::schema::SCHEMA_SESSION,
            cafe_types::schema::SCHEMA_SESSION,
            SessionConfig::default(),
        )
        .await
    {
        Ok(()) => {}
        Err(crate::SdkError::BusError {
            ref message,
            code: Some(ref c),
        }) if c == "SESSION_EXISTS" => {}
        Err(e) => return Err(e),
    }

    let chunk = Chunk::new_null("cafe-sdk")
        .with_annotation(keys::CAFE_SCHEMA_EVALUATOR, &schema);
    bus.publish(cafe_types::schema::SCHEMA_SESSION, chunk).await?;

    Ok(())
}
