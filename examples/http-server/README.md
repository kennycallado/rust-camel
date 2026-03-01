# HTTP Server Example - REST API

A realistic REST API example demonstrating rust-camel's HTTP server capabilities.

## Features

- **Health & Metrics**: Service health check with uptime and request metrics
- **User Management**: Full CRUD-style user endpoints
- **Text Processing**: Text transformation with detailed statistics
- **Utilities**: Server time in multiple formats

## Running

```bash
cargo run -p http-server
```

## API Endpoints

### Health & Metrics

```bash
GET /health
```

Returns service health, version, uptime, and request count.

**Example:**
```bash
curl http://localhost:8080/health | jq
```

**Response:**
```json
{
  "status": "UP",
  "service": "rust-camel-api",
  "version": "1.0.0",
  "uptime_seconds": 12345,
  "requests_processed": 42,
  "timestamp": 1709251200
}
```

---

### User Management

#### List All Users

```bash
GET /api/users
```

Returns paginated list of users (mock data).

**Example:**
```bash
curl http://localhost:8080/api/users | jq
```

**Response:**
```json
{
  "users": [
    {"id": 1, "name": "Alice", "email": "alice@example.com", "role": "admin"},
    {"id": 2, "name": "Bob", "email": "bob@example.com", "role": "user"},
    {"id": 3, "name": "Charlie", "email": "charlie@example.com", "role": "user"}
  ],
  "total": 3,
  "page": 1,
  "per_page": 10
}
```

---

#### Create User

```bash
POST /api/users/create
Content-Type: application/json

{
  "name": "John",
  "email": "john@example.com"
}
```

Creates a new user with auto-generated ID and timestamp.

**Example:**
```bash
curl -X POST http://localhost:8080/api/users/create \
     -d '{"name":"John","email":"john@example.com"}' | jq
```

**Response:** `201 Created`
```json
{
  "id": 1709251234567890123,
  "name": "John",
  "email": "john@example.com",
  "role": "user",
  "created_at": 1709251200,
  "status": "created"
}
```

---

#### Get User by ID

```bash
GET /api/users/id?id={id}
```

Returns user details by ID (try id=1, 2, or 3 for existing users).

**Example:**
```bash
curl 'http://localhost:8080/api/users/id?id=1' | jq
```

**Response:** `200 OK`
```json
{
  "id": 1,
  "name": "Alice",
  "email": "user1@example.com",
  "role": "admin"
}
```

**Not Found:** `404 Not Found`
```json
{
  "error": "User not found",
  "id": 999
}
```

---

### Text Processing

#### Transform Text

```bash
POST /api/transform

Any text content
```

Transforms text with detailed statistics (character count, words, case analysis, etc).

**Example:**
```bash
curl -X POST http://localhost:8080/api/transform \
     -d 'Hello World 2024!' | jq
```

**Response:**
```json
{
  "original": "Hello World 2024!",
  "transformed": "HELLO WORLD 2024!",
  "reversed": "!4202 dlroW olleH",
  "statistics": {
    "characters": 17,
    "words": 3,
    "uppercase": 2,
    "lowercase": 8,
    "digits": 4,
    "spaces": 2
  },
  "metadata": {
    "processed_at": 1709251200,
    "version": "1.0"
  }
}
```

---

### Utilities

#### Server Time

```bash
GET /api/time
```

Returns current server time in multiple formats.

**Example:**
```bash
curl http://localhost:8080/api/time | jq
```

**Response:**
```json
{
  "unix_timestamp": 1709251200,
  "iso8601": "2024-03-01T12:00:00Z",
  "timezone": "UTC"
}
```

---

## Architecture

This example demonstrates:

- **Multiple HTTP endpoints** on the same port (via ServerRegistry)
- **Request processing** with body parsing and transformation
- **Response customization** with headers and status codes
- **Shared state** across routes (request counter)
- **Error handling** (404 for missing users)
- **Header filtering** (automatic removal of hop-by-hop headers)

## Testing

Run the server and try the example commands shown in each endpoint section above.

All endpoints return JSON and work with `jq` for pretty printing:

```bash
# Quick test all endpoints
curl http://localhost:8080/health | jq
curl http://localhost:8080/api/users | jq
curl -X POST http://localhost:8080/api/users/create -d '{"name":"Test","email":"test@example.com"}' | jq
curl 'http://localhost:8080/api/users/id?id=1' | jq
curl -X POST http://localhost:8080/api/transform -d 'Sample text 123' | jq
curl http://localhost:8080/api/time | jq
```
