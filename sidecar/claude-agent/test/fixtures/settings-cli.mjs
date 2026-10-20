// SDK control-transport fixture. No model, credentials or network.
import { createInterface } from "node:readline";
for await (const line of createInterface({ input: process.stdin })) {
  const message = JSON.parse(line);
  if (message.type !== "control_request") continue;
  const settings = message.request.subtype === "get_settings";
  const response = settings
    ? {
        effective: {
          permissions: {
            deny: ["mcp__codemux__fixture__blocked"],
            allow: ["mcp__codemux__fixture__allowed"],
          },
        },
        sources: [],
        applied: {},
      }
    : {
        commands: [],
        agents: [],
        output_style: "default",
        available_output_styles: [],
        models: [],
        account: {},
      };
  console.log(
    JSON.stringify({
      type: "control_response",
      response: {
        subtype: "success",
        request_id: message.request_id,
        response,
      },
    }),
  );
}
