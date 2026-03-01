# HTTP Server Example - Realistic REST API

A production-ready REST API example demonstrating rust-camel's HTTP server capabilities with proper validation, error handling, and in-memory database.

## Features

- **Full CRUD Operations** - Create, Read, Update, Delete users
- **Validation** - Comprehensive input validation with meaningful error messages
- **Error Handling** - Proper HTTP status codes (400, 404, 409, 500)
- **In-Memory Database** - Thread-safe storage using DashMap
- **Pagination** - List endpoints support page and per_page parameters
- **Metrics** - Health endpoint with request count and user count

## Running

```bash
cargo run -p http-server
```

## API Endpoints

### Health Check

```bash
GET /health
```

Returns service health with metrics.

**Example:**
```bash
curl http://localhost:8080/health | jq
```

**Response:** `200 OK`
```json
{
  "status": "UP",
  "service": "rust-camel-api",
  "version": "1.0.0",
  "uptime_seconds": 12345,
  "requests_processed": 42,
  "users_count": 5,
  "timestamp": 1709251200
}
```

---

### List Users

```bash
GET /api/users?page={page}&per_page={per_page}
```

Returns paginated list of users.

**Parameters:**
- `page` (optional, default: 1) - Page number (min: 1)
- `per_page` (optional, default: 10, max: 100) - Users per page

**Example:**
```bash
curl 'http://localhost:8080/api/users?page=1&per_page=10' | jq
```

**Response:** `200 OK`
```json
{
  "users": [
    {
      "id": 1,
      "name": "Alice",
      "email": "alice@example.com",
      "age": 30,
      "role": "admin",
      "created_at": 1709251200,
      "updated_at": null
    }
  ],
  "total": 1,
  "page": 1,
  "per_page": 10
}
```

---

### Create User

```bash
POST /api/users/create
Content-Type: application/json

{
  "name": "Alice",
  "email": "alice@example.com",
  "age": 30,
  "role": "admin"
}
```

Creates a new user with validation.

**Fields:**
- `name` (required, max: 100 chars) - User's name
- `email` (required, valid email format) - Must be unique
- `age` (optional, max: 150) - User's age
- `role` (optional, default: "user") - One of: "user", "admin", "moderator"

**Example:**
```bash
curl -X POST http://localhost:8080/api/users/create \
     -H 'Content-Type: application/json' \
     -d '{"name":"Alice","email":"alice@example.com","age":30,"role":"admin"}' | jq
```

**Response:** `201 Created`
```json
{
  "id": 1,
  "name": "Alice",
  "email": "alice@example.com",
  "age": 30,
  "role": "admin",
  "created_at": 1709251200,
  "updated_at": null
}
```

**Errors:**
- `400 Bad Request` - Invalid input (validation error)
- `409 Conflict` - Email already exists

```json
{
  "error": "Email already exists",
  "code": 409
}
```

---

### Get User

```bash
GET /api/users/id?id={id}
```

Returns user details by ID.

**Parameters:**
- `id` (required) - User ID

**Example:**
```bash
curl 'http://localhost:8080/api/users/id?id=1' | jq
```

**Response:** `200 OK`
```json
{
  "id": 1,
  "name": "Alice",
  "email": "alice@example.com",
  "age": 30,
  "role": "admin",
  "created_at": 1709251200,
  "updated_at": null
}
```

**Errors:**
- `404 Not Found` - User doesn't exist

```json
{
  "error": "User 999 not found",
  "code": 404
}
```

---

### Update User

```bash
PUT /api/users/update?id={id}
Content-Type: application/json

{
  "name": "New Name",
  "age": 35
}
```

Updates user with partial data. Only provided fields are updated.

**Parameters:**
- `id` (required) - User ID

**Fields:** (all optional)
- `name` (max: 100 chars) - User's name
- `email` (valid email format, must be unique)
- `age` (max: 150) - User's age
- `role` - One of: "user", "admin", "moderator"

**Example:**
```bash
curl -X PUT 'http://localhost:8080/api/users/update?id=1' \
     -H 'Content-Type: application/json' \
     -d '{"age":35,"role":"moderator"}' | jq
```

**Response:** `200 OK`
```json
{
  "id": 1,
  "name": "Alice",
  "email": "alice@example.com",
  "age": 35,
  "role": "moderator",
  "created_at": 1709251200,
  "updated_at": 1709251300
}
```

**Errors:**
- `400 Bad Request` - Invalid input or no fields to update
- `404 Not Found` - User doesn't exist

---

### Delete User

```bash
DELETE /api/users/delete?id={id}
```

Deletes user by ID.

**Parameters:**
- `id` (required) - User ID

**Example:**
```bash
curl -X DELETE 'http://localhost:8080/api/users/delete?id=1' | jq
```

**Response:** `200 OK`
```json
{
  "message": "User deleted successfully",
  "user": {
    "id": 1,
    "name": "Alice",
    "email": "alice@example.com",
    "age": 35,
    "role": "moderator",
    "created_at": 1709251200,
    "updated_at": 1709251300
  }
}
```

**Errors:**
- `404 Not Found` - User doesn't exist

---

## Architecture

This example demonstrates:

- **In-memory storage** - Thread-safe DashMap for concurrent access
- **Validation** - Comprehensive input validation before processing
- **Error handling** - Proper HTTP status codes and error messages
- **Partial updates** - Only update provided fields
- **Auto-generated IDs** - Atomic counter for unique IDs
- **Timestamps** - Auto-managed created_at and updated_at fields

## Testing

### Unit Tests

```bash
cargo test -p http-server
```

### Integration Tests

Run the server and try the example commands shown in each endpoint section above.

**Quick test script:**
```bash
# Create
curl -X POST http://localhost:8080/api/users/create \
     -H 'Content-Type: application/json' \
     -d '{"name":"Alice","email":"alice@example.com","age":30}' | jq

# List
curl 'http://localhost:8080/api/users' | jq

# Get
curl 'http://localhost:8080/api/users/id?id=1' | jq

# Update
curl -X PUT 'http://localhost:8080/api/users/update?id=1' \
     -H 'Content-Type: application/json' \
     -d '{"age":35}' | jq

# Delete
curl -X DELETE 'http://localhost:8080/api/users/delete?id=1' | jq
```

## Implementation Notes

### Body Handling

HTTP request bodies are automatically converted to UTF-8 text when valid, allowing easy parsing with `as_text()`. Binary data is kept as bytes.

### Concurrency

The in-memory database uses `DashMap` for thread-safe concurrent access without locking the entire map.

### Validation

All input is validated before processing:
- Required fields are checked
- String lengths are limited
- Email format is validated (basic check)
- Enum values are validated
- Age range is checked

### Error Responses

All errors follow a consistent format:
```json
{
  "error": "Human-readable error message",
  "code": 400,
  "details": ["Optional", "array", "of", "details"]
}
```
