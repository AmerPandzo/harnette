// Self-check for the chat's action parser: `node scripts/check-actions.ts`
import assert from "node:assert/strict";
import { segments, toAction } from "../src/lib/actions.ts";

const text = 'Plan:\n```kormilant-action\n{"action":"run","slug":"T-1"}\n```\nthen\n```kormilant-action\n{"action":"move","slug":"T-1","status":"nope"}\n```';
const s = segments(text);
assert.deepEqual(s[0], { t: "md", text: "Plan:\n" });
assert.deepEqual(s[1], { t: "action", action: { action: "run", slug: "T-1" } });
assert.deepEqual(s[2], { t: "md", text: "\nthen\n" });
assert.equal(s[3].t, "bad");
assert.deepEqual(segments("no actions"), [{ t: "md", text: "no actions" }]);

assert.equal(toAction('{"action":"run","slug":"../x"}'), "missing or invalid slug");
assert.equal(toAction('{"action":"analyze","slug":"N-1"}'), "analyze needs a ticket");
assert.deepEqual(toAction('{"action":"refine","slug":"T"}'), { action: "refine", slug: "T", message: "" });
assert.deepEqual(toAction('{"action":"move","slug":"T","status":"done"}'), { action: "move", slug: "T", status: "done" });
assert.equal(toAction("{"), "not valid JSON");
assert.match(toAction('{"action":"rm","slug":"T"}') as string, /unknown action/);
console.log("actions: ok");
