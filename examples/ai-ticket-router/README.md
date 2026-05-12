# AI Ticket Router — Customer Support Pipeline

A realistic customer support pipeline that sends 5 diverse support tickets through AI-powered extraction, classification, and content-based routing to specialized teams.

## Architecture

```
                          ┌─────────────────────┐
  Timer:billing ────────► │                     │
  Timer:technical ───────► │  direct:process-    │
  Timer:account ─────────► │     ticket          │
  Timer:escalation ──────► │                     │
  Timer:spam ────────────► │  ┌───────────────┐  │
                          │  │  ai_extract    │──┼──► header.extracted (JSON)
                          │  └───────┬───────┘  │
                          │          ▼          │
                          │  ┌───────────────┐  │
                          │  │  ai_classify   │──┼──► header.category (label)
                          │  └───────┬───────┘  │
                          │          ▼          │
                          │  ┌───────────────┐  │
                          │  │     log        │  │
                          │  └───────┬───────┘  │
                          │          ▼          │
                          │  ┌───────────────┐  │
                          │  │    choice      │  │
                          │  └──┬──┬──┬──┬────┘  │
                          └─────┼──┼──┼──┼───────┘
                                │  │  │  │
                 ┌──────────────┘  │  │  └──────────────┐
                 ▼                 ▼  ▼                  ▼
          direct:billing   direct:technical   direct:account   direct:general
          "Routed to       "Routed to        "Routed to       "Routed to
           billing: ..."    technical: ..."    account: ..."    general: ..."
```

## Prerequisites

- [Ollama](https://ollama.ai) running at `localhost:11434`
- Model pulled:

```bash
ollama pull qwen3.5:4b
```

## Run

```bash
cargo run -p ai-ticket-router
```

## Expected Output

```
ai-ticket-router: Customer Support Pipeline Demo
================================================
Loaded 10 route(s) from YAML
Added route: billing-ticket
Added route: technical-ticket
Added route: account-ticket
Added route: escalation-ticket
Added route: spam-ticket
Added route: process-ticket
Added route: billing-team
Added route: technical-team
Added route: account-team
Added route: general-team

Category: billing | Priority: {"customer_name":null,"email":null,"priority":"high","summary":"Double charge on subscription"}
Routed to billing: I was charged twice for my subscription last month...

Category: technical | Priority: {"customer_name":null,"email":null,"priority":"high","summary":"App crashes on large file upload"}
Routed to technical: The app crashes every time I try to upload...

Category: account | Priority: {"customer_name":null,"email":"maria.garcia@example.com","priority":"medium","summary":"Cannot log in, reset email not received"}
Routed to account: I can't log into my account...

Category: uncategorized | Priority: {"customer_name":null,"email":null,"priority":"critical","summary":"Third contact, demanding manager escalation"}
Routed to general: This is the third time I'm contacting support...

Category: spam | Priority: {"customer_name":null,"email":null,"priority":"low","summary":"Spam crypto mining advertisement"}
Routed to general: Hey check out this amazing deal...
```

## How It Works

### 1. Ticket Generation (Timer Routes)

Five timer routes fire once with staggered delays (100ms–500ms), each setting a realistic support ticket as the message body and routing to the shared processing pipeline.

### 2. ai_extract

Extracts structured data from the ticket text into a JSON object with fields: `customer_name`, `email`, `priority`, and `summary`. The result is stored in the `extracted` header.

### 3. ai_classify

Classifies each ticket into one of four categories: `billing`, `technical`, `account`, or `spam`. Unknown inputs fall back to `uncategorized`. The label is stored in the `category` header.

### 4. choice

Content-based router reads the `category` header and dispatches to the matching team endpoint (`direct:billing-team`, `direct:technical-team`, `direct:account-team`), with everything else going to `direct:general-team`.

### 5. Team Endpoints

Each team route logs the routed ticket with a team-specific prefix.

## See Also

- [other examples](../) — more rust-camel demos
