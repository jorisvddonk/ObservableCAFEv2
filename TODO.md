# TODO

- Switch cafe-agent-runtime from 2s polling to SubscribeAll (like cafe-llm).
  Currently cafe-llm discovers sessions instantly, but cafe-agent-runtime is
  still on 2s polling. This means cafe-llm subscribes before the pipeline
  discovers the session, masking the transient-RPC race. If cafe-agent-runtime
  also goes SubscribeAll, the race reappears — both get SessionCreated at the
  same time and the transient RPC could be dispatched before cafe-llm's
  run_session subscribes.
