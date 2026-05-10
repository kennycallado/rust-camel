export interface PatchWire {
  body?: { kind: string; value?: unknown };
  headers_set?: Array<[string, unknown]>;
  headers_removed?: string[];
  properties_set?: Array<[string, unknown]>;
}

export type CamelHandler = (camel: Camel) => void | Promise<void>;

export class Camel {
  private _body: string | object | null;
  private _headers: Record<string, unknown>;
  private _properties: Record<string, unknown>;
  private _patch: PatchWire;

  constructor(
    body: string | object | null,
    headers: Record<string, unknown>,
    properties: Record<string, unknown>
  ) {
    this._body = body;
    this._headers = { ...headers };
    this._properties = { ...properties };
    this._patch = {};
  }

  body(): string | object | null {
    return this._body;
  }

  setBody(value: string | object | null): void {
    this._body = value;
    if (value === null) {
      this._patch.body = { kind: "empty" };
    } else if (typeof value === "string") {
      this._patch.body = { kind: "text", value };
    } else {
      this._patch.body = { kind: "json", value };
    }
  }

  header(name: string): unknown {
    return this._headers[name];
  }

  setHeader(name: string, value: unknown): void {
    this._headers[name] = value;
    if (!this._patch.headers_set) this._patch.headers_set = [];
    const idx = this._patch.headers_set.findIndex(([k]) => k === name);
    if (idx >= 0) {
      this._patch.headers_set[idx] = [name, value];
    } else {
      this._patch.headers_set.push([name, value]);
    }
    if (this._patch.headers_removed) {
      this._patch.headers_removed = this._patch.headers_removed.filter((h) => h !== name);
    }
  }

  removeHeader(name: string): void {
    delete this._headers[name];
    if (!this._patch.headers_removed) this._patch.headers_removed = [];
    if (!this._patch.headers_removed.includes(name)) {
      this._patch.headers_removed.push(name);
    }
    if (this._patch.headers_set) {
      this._patch.headers_set = this._patch.headers_set.filter(([k]) => k !== name);
    }
  }

  property(name: string): unknown {
    return this._properties[name];
  }

  setProperty(name: string, value: unknown): void {
    this._properties[name] = value;
    if (!this._patch.properties_set) this._patch.properties_set = [];
    const idx = this._patch.properties_set.findIndex(([k]) => k === name);
    if (idx >= 0) {
      this._patch.properties_set[idx] = [name, value];
    } else {
      this._patch.properties_set.push([name, value]);
    }
  }

  collectPatch(): PatchWire {
    return this._patch;
  }
}
