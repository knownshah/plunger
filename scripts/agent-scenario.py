"""Drives Plunger the way an agent does, against scripts/tracker-api.py, and checks what comes back.

Run:  python scripts/agent-scenario.py path/to/plunger[.exe]

It starts the fake tracker on a free port and uses a throwaway data folder, so it touches nothing of
yours. It exists because real use found problems our unit tests did not: text with line breaks and
quotes in a JSON body, optional fields, error pages that are not JSON, nulls, and saved requests.
"""
import json
import os
import socket
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
PLUNGER = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else "plunger"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


PORT = free_port()
BASE = f"http://127.0.0.1:{PORT}"
DATA = tempfile.mkdtemp(prefix="plunger-scenario-")
ENV = dict(os.environ, PLUNGER_DATA_DIR=DATA)
failures = []


def run(*args, expect_ok=True):
    r = subprocess.run([PLUNGER, *args], capture_output=True, text=True, encoding="utf-8", env=ENV, timeout=60)
    try:
        out = json.loads(r.stdout) if r.stdout.strip() else None
    except ValueError:
        out = {"_raw": r.stdout}
    if expect_ok and r.returncode != 0:
        print(f"   ! plunger {' '.join(args)[:100]} exited {r.returncode}: {r.stdout[:300]} {r.stderr[:200]}")
    return out


def check(name, condition, detail=""):
    print(("ok   " if condition else "FAIL ") + name + ("" if condition else f"   {detail}"))
    if not condition:
        failures.append(name)


server = subprocess.Popen([sys.executable, os.path.join(HERE, "tracker-api.py"), str(PORT)])
try:
    for _ in range(50):
        try:
            socket.create_connection(("127.0.0.1", PORT), timeout=0.2).close()
            break
        except OSError:
            time.sleep(0.1)

    run("vars", "set", "base", BASE)
    run("vars", "set", "tracker_key", "tok_tracker")
    auth = ["-H", "Authorization: Bearer {{tracker_key}}"]

    # 1. text a person writes: line breaks, quotes and a backslash must arrive exactly as written
    text = 'First line\nSecond "quoted" line\nC:\\temp\\new and a tab\there'
    created = run("send", "--url", "{{base}}/issue", "-X", "POST", *auth,
                  "--json", '{"fields": {"summary": "Bug {{n}}", "description": "{{description}}"}}',
                  "--var", "n=1", "--var", f"description={text}", "--select", "$.key", "--select", "header:Location")
    check("create with awkward text is accepted", created and created.get("status") == 201, created)
    key = (created or {}).get("selected", {}).get("$.key")
    check("select leaves the header noise out", created and not created.get("headers"), created and created.get("headers"))
    issue = run("send", "--url", f"{BASE}/issue/{key}", *auth, "--select", "$.fields.description")
    check("the text came back exactly", issue and issue["selected"]["$.fields.description"] == text, issue)

    # 2. optional fields: left out when there is no value, sent when there is
    optional = '{"fields": {"summary": "S", "assignee": {"name": "{{who?}}"}, "labels": {{labels?}}, "epic": "{{epic?}}"}}'
    bare = run("send", "--url", "{{base}}/issue", "-X", "POST", *auth, "--json", optional)
    check("optional fields are left out (an empty assignee would be a 400)", bare and bare.get("status") == 201, bare)
    full = run("send", "--url", "{{base}}/issue", "-X", "POST", *auth, "--json", optional,
               "--var", "who=ann", "--var", 'labels=["a", "b"]', "--var", "epic=E-1")
    check("optional fields are sent when given (labels as a real array)", full and full.get("status") == 201, full)
    got = run("send", "--url", f"{BASE}/issue/{full['json']['key']}", *auth, "--select", "$.fields.labels", "--select", "$.fields.assignee.displayName")
    check("the array and the assignee arrived", got and got["selected"] == {"$.fields.labels": ["a", "b"], "$.fields.assignee.displayName": "ann"}, got)

    # 3. a null parent is null, not a problem
    unassigned = run("send", "--url", f"{BASE}/issue/{(bare or {}).get('json', {}).get('key')}", *auth, "--select", "$.fields.assignee.displayName")
    check("a path below a null is null with no problem", unassigned and unassigned["selected"] == {"$.fields.assignee.displayName": None} and not unassigned.get("problems"), unassigned)

    # 4. an error page that is not JSON: the whole response comes back, once
    boom = run("send", "--url", f"{BASE}/boom", *auth, "--select", "$.key", expect_ok=False)
    check("a failed select returns the body and says why", boom and "Internal Server Error" in (boom.get("body") or "") and boom.get("problems"), boom)

    # 5. a saved request keeps its timeout and select
    run("save", "create issue", "--url", "{{base}}/issue", "-X", "POST", *auth, "--json", '{"fields": {"summary": "{{summary}}"}}',
        "--timeout", "90", "--select", "$.key")
    shown = run("saved", "show", "create issue")
    check("saved request shows its options", shown and shown.get("options", {}).get("timeout_secs") == 90 and shown["options"].get("select") == ["$.key"], shown)
    again = run("send", "create issue", "--var", "summary=From a saved request")
    check("sending it by name uses the saved select", again and again.get("selected", {}).get("$.key", "").startswith("PRJ-"), again)

    # 6. workflow: find a transition by name, apply it, look something up that is not there
    steps = [
        {"label": "transitions", "url": "{{base}}/issue/{{key}}/transitions", "headers": {"Authorization": "Bearer {{tracker_key}}"},
         "extract": [{"name": "transition_id", "from": "json:$.transitions[?(@.name=='Done')].id", "scope": "run"}]},
        {"label": "move", "method": "POST", "url": "{{base}}/issue/{{key}}/transitions", "headers": {"Authorization": "Bearer {{tracker_key}}"},
         "json": {"transition": {"id": "{{transition_id}}"}}, "expect_status": 204},
        {"label": "look for a missing one", "url": "{{base}}/issue/PRJ-999", "headers": {"Authorization": "Bearer {{tracker_key}}"},
         "expect_status": [200, 404], "extract": [{"name": "keep_me", "from": "status"}]},
        {"label": "create it only if it was found", "method": "POST", "url": "{{base}}/issue", "headers": {"Authorization": "Bearer {{tracker_key}}"},
         "json": {"fields": {"summary": "never"}}, "skip_if": "{{previous_status}} == 404"},
    ]
    step_file = os.path.join(DATA, "steps.json")
    with open(step_file, "w", encoding="utf-8") as f:
        json.dump(steps, f)
    run("workflow", "save", "close issue", "@" + step_file)
    result = run("workflow", "run", "close issue", "--var", f"key={key}", "--cleanup")
    check("workflow ran: transition by name, 404 accepted, create skipped",
          result and result.get("ok") and result["steps"][3].get("skipped") and result["steps"][1]["status"] == 204, result)
    status = run("send", "--url", f"{BASE}/issue/{key}", *auth, "--select", "$.fields.status.name")
    check("the issue is Done", status and status["selected"]["$.fields.status.name"] == "Done", status)
    names = [v["name"] for v in (run("vars") or {}).get("variables", [])]
    check("cleanup left no workflow variables behind", "keep_me" not in names and "transition_id" not in names, names)

    # 7. a value cannot become another header
    inject = run("send", "--url", "{{base}}/search", *auth, "-H", "X-Trace: {{trace}}", "--var", "trace=ok\nX-Evil: 1", expect_ok=False)
    check("a header value with a line break is refused, not split", inject and inject.get("kind") == "not_sent", inject)
finally:
    server.terminate()

print()
if failures:
    print(f"{len(failures)} check(s) failed: {failures}")
    sys.exit(1)
print("all checks passed")
