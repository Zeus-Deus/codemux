# Async question verification

All screenshots use repository development fixtures. They contain no live conversation, credentials, or private project content.

- `before-callback.png`: unchanged `main`, existing callback questionnaire (`?askq=1`).
- `after-pending.png`: native asynchronous question fixture (`?asyncq=1`), with the composer available and work continuing.
- `after-answered.png`: accepted answer and subsequent assistant response in the same turn.

The UI was checked on ports 1421 and 1422 because port 1420 was already occupied. The live provider check used an empty temporary project, a synthetic storage choice, Codex 0.153.2, and GPT-6 Astra. It verified that a native question arrived, independent work started before the answer, `turn/steer` accepted the answer in the original turn, and history contained one correlated answer. A completed turn rejected steering with `-32600: no active turn to steer`; a late answer through `turn/start` produced one new turn and one correlated message. Raw provider logs and authentication files are excluded.
