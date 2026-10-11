// modebox prelude: evaluated once at pre-initialization, so its cost is baked
// into the snapshot image and never paid per run. `al` holds the native
// bridge; it is captured here and never reachable from guest code.
(function (al) {
  'use strict';
  const g = globalThis;
  const _Map = Map, _Set = Set, _RegExp = RegExp, _Error = Error, _BigInt = BigInt;
  const _Uint8Array = Uint8Array, _isView = ArrayBuffer.isView;
  const _apply = Reflect.apply, _define = Object.defineProperty;
  const _mapForEach = Map.prototype.forEach, _setForEach = Set.prototype.forEach;
  // Append without [[Set]], so setters on Array.prototype can't intercept.
  const push = (a, v) => _define(a, a.length, { value: v, writable: true, enumerable: true, configurable: true });
  const errorTypes = { Error, TypeError, RangeError, SyntaxError, ReferenceError, EvalError, URIError };

  const hidden = (obj, name, value) =>
    Object.defineProperty(obj, name, { value, writable: true, configurable: true, enumerable: false });
  const visible = (obj, name, value) =>
    Object.defineProperty(obj, name, { value, writable: true, configurable: true, enumerable: true });

  // -- console --------------------------------------------------------------
  const console = {};
  for (const level of ['log', 'info', 'warn', 'error', 'debug', 'trace']) {
    visible(console, level, (...args) => al.log(level, ...args));
  }
  visible(console, 'dir', (v) => al.log('log', v));
  visible(console, 'table', (v) => al.log('log', v));
  visible(console, 'assert', (cond, ...args) => {
    if (!cond) al.log('error', 'Assertion failed' + (args.length ? ':' : ''), ...args);
  });
  const counts = new _Map();
  visible(console, 'count', (label = 'default') => {
    const n = (counts.get(label) || 0) + 1;
    counts.set(label, n);
    al.log('log', `${label}: ${n}`);
  });
  const times = new _Map();
  visible(console, 'time', (label = 'default') => { times.set(label, Date.now()); });
  visible(console, 'timeEnd', (label = 'default') => {
    if (!times.has(label)) return;
    al.log('log', `${label}: ${Date.now() - times.get(label)}ms`);
    times.delete(label);
  });
  hidden(g, 'console', console);

  // -- randomness: seeded per run by the host --------------------------------
  hidden(Math, 'random', () => al.random());
  const bytesOf = (view) => new _Uint8Array(view.buffer, view.byteOffset, view.byteLength);
  hidden(g, 'crypto', {
    getRandomValues(view) {
      if (!_isView(view)) throw new TypeError('crypto.getRandomValues: expected a TypedArray');
      if (view.byteLength > 65536) throw new RangeError('crypto.getRandomValues: quota exceeded (65536 bytes)');
      al.fillRandom(bytesOf(view));
      return view;
    },
    randomUUID() {
      const b = new _Uint8Array(16);
      al.fillRandom(b);
      b[6] = (b[6] & 0x0f) | 0x40;
      b[8] = (b[8] & 0x3f) | 0x80;
      let h = '';
      for (let i = 0; i < 16; i++) h += (b[i] < 16 ? '0' : '') + b[i].toString(16);
      return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
    },
  });

  // -- encoding ---------------------------------------------------------------
  class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(input = '') { return al.utf8Encode(String(input)); }
    encodeInto(input, dest) {
      const bytes = al.utf8Encode(String(input));
      const n = Math.min(bytes.length, dest.length);
      dest.set(bytes.subarray(0, n));
      return { read: n === bytes.length ? String(input).length : al.utf8Decode(bytes.subarray(0, n), false).length, written: n };
    }
  }
  class TextDecoder {
    #fatal;
    constructor(label = 'utf-8', options = {}) {
      if (!/^utf-?8$/i.test(String(label))) throw new RangeError(`TextDecoder: unsupported encoding "${label}"`);
      this.#fatal = !!options.fatal;
    }
    get encoding() { return 'utf-8'; }
    get fatal() { return this.#fatal; }
    decode(input) {
      if (input === undefined) return '';
      const bytes = input instanceof _Uint8Array ? input : _isView(input) ? bytesOf(input) : new _Uint8Array(input);
      return al.utf8Decode(bytes, this.#fatal);
    }
  }
  hidden(g, 'TextEncoder', TextEncoder);
  hidden(g, 'TextDecoder', TextDecoder);
  hidden(g, 'atob', (s) => al.atob(String(s)));
  hidden(g, 'btoa', (s) => al.btoa(String(s)));
  hidden(g, 'structuredClone', (v) => al.clone(v));

  // -- URL and URLSearchParams (WHATWG; parsing in Rust, the `url` crate) ----
  // Values that cross the boundary as something else: a URL as its href.
  const ENCODE = Symbol('modebox.encode');
  let attachParams, resetParams, setSearch;
  class URLSearchParams {
    #list = [];
    #url = null;
    static {
      attachParams = (p, url) => { p.#url = url; };
      resetParams = (p, search) => { p.#list = al.formParse(search.startsWith('?') ? search.slice(1) : search); };
    }
    constructor(init = '') {
      if (init instanceof URLSearchParams) {
        this.#list = init.#list.map(([k, v]) => [k, v]);
      } else if (init !== null && typeof init === 'object') {
        if (typeof init[Symbol.iterator] === 'function') {
          for (const pair of init) {
            const kv = [...pair];
            if (kv.length !== 2) throw new TypeError('URLSearchParams: each pair needs exactly a name and a value');
            this.#list.push([String(kv[0]), String(kv[1])]);
          }
        } else {
          for (const k of Object.keys(init)) this.#list.push([k, String(init[k])]);
        }
      } else {
        resetParams(this, String(init));
      }
    }
    #changed() { if (this.#url) setSearch(this.#url, this.toString()); }
    get size() { return this.#list.length; }
    append(name, value) { this.#list.push([String(name), String(value)]); this.#changed(); }
    delete(name, value) {
      name = String(name);
      this.#list = this.#list.filter(([k, v]) => !(k === name && (value === undefined || v === String(value))));
      this.#changed();
    }
    get(name) { name = String(name); const e = this.#list.find(([k]) => k === name); return e ? e[1] : null; }
    getAll(name) { name = String(name); return this.#list.filter(([k]) => k === name).map(([, v]) => v); }
    has(name, value) { name = String(name); return this.#list.some(([k, v]) => k === name && (value === undefined || v === String(value))); }
    set(name, value) {
      name = String(name); value = String(value);
      const i = this.#list.findIndex(([k]) => k === name);
      if (i < 0) this.#list.push([name, value]);
      else this.#list = this.#list.filter(([k], j) => j <= i || k !== name).map((e, j) => (j === i ? [name, value] : e));
      this.#changed();
    }
    sort() {
      // Stable, by UTF-16 code units, as the standard says.
      this.#list = this.#list.map((e, i) => [e, i]).sort(([a, i], [b, j]) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : i - j)).map(([e]) => e);
      this.#changed();
    }
    forEach(fn, thisArg) { for (const [k, v] of this.#list) fn.call(thisArg, v, k, this); }
    *entries() { for (const [k, v] of this.#list) yield [k, v]; }
    *keys() { for (const [k] of this.#list) yield k; }
    *values() { for (const [, v] of this.#list) yield v; }
    [Symbol.iterator]() { return this.entries(); }
    toString() { return al.formSerialize(this.#list); }
    [ENCODE]() { return this.toString(); }
    get [Symbol.toStringTag]() { return 'URLSearchParams'; }
  }

  const PARTS = ['protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash'];
  class URL {
    #r;
    #params = null;
    static {
      setSearch = (u, query) => { u.#r = al.urlSet(u.#r.href, 'search', query) ?? u.#r; };
    }
    constructor(url, base) {
      const r = al.urlParse(String(url), base === undefined ? undefined : String(base));
      if (!r) throw new TypeError(`Invalid URL: ${String(url)}`);
      this.#r = r;
    }
    static canParse(url, base) { return al.urlParse(String(url), base === undefined ? undefined : String(base)) !== null; }
    static parse(url, base) { try { return new URL(url, base); } catch { return null; } }
    get href() { return this.#r.href; }
    set href(v) {
      const r = al.urlParse(String(v));
      if (!r) throw new TypeError(`Invalid URL: ${String(v)}`);
      this.#r = r;
      if (this.#params) resetParams(this.#params, r.search);
    }
    get origin() { return this.#r.origin; }
    get searchParams() {
      if (!this.#params) {
        this.#params = new URLSearchParams(this.#r.search);
        attachParams(this.#params, this);
      }
      return this.#params;
    }
    toString() { return this.#r.href; }
    toJSON() { return this.#r.href; }
    [ENCODE]() { return this.#r.href; }
    get [Symbol.toStringTag]() { return 'URL'; }
    static {
      for (const part of PARTS) {
        Object.defineProperty(URL.prototype, part, {
          get() { return this.#r[part]; },
          set(v) {
            this.#r = al.urlSet(this.#r.href, part, String(v)) ?? this.#r;
            if (part === 'search' && this.#params) resetParams(this.#params, this.#r.search);
          },
          enumerable: true,
          configurable: true,
        });
      }
    }
  }
  hidden(g, 'URL', URL);
  hidden(g, 'URLSearchParams', URLSearchParams);

  // -- fetch: requests made by the host's `$fetch` tool, under its policy ----
  const HEADER_NAME = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;
  class Headers {
    #map = new _Map(); // lowercased name -> value
    constructor(init) {
      if (init instanceof Headers) for (const [k, v] of init) this.append(k, v);
      else if (init !== null && typeof init === 'object') {
        if (typeof init[Symbol.iterator] === 'function') {
          for (const pair of init) {
            const kv = [...pair];
            if (kv.length !== 2) throw new TypeError('Headers: each pair needs exactly a name and a value');
            this.append(kv[0], kv[1]);
          }
        } else for (const k of Object.keys(init)) this.append(k, init[k]);
      }
    }
    static #name(n) {
      n = String(n);
      if (!HEADER_NAME.test(n)) throw new TypeError(`Headers: invalid header name "${n}"`);
      return n.toLowerCase();
    }
    static #value(v) {
      v = String(v).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, '');
      if (/[\0\r\n]/.test(v)) throw new TypeError('Headers: invalid header value');
      return v;
    }
    append(n, v) { n = Headers.#name(n); v = Headers.#value(v); const e = this.#map.get(n); this.#map.set(n, e === undefined ? v : `${e}, ${v}`); }
    set(n, v) { this.#map.set(Headers.#name(n), Headers.#value(v)); }
    get(n) { const v = this.#map.get(Headers.#name(n)); return v === undefined ? null : v; }
    has(n) { return this.#map.has(Headers.#name(n)); }
    delete(n) { this.#map.delete(Headers.#name(n)); }
    getSetCookie() { const v = this.#map.get('set-cookie'); return v === undefined ? [] : v.split(', '); }
    *entries() { for (const k of [...this.#map.keys()].sort()) yield [k, this.#map.get(k)]; }
    *keys() { for (const [k] of this.entries()) yield k; }
    *values() { for (const [, v] of this.entries()) yield v; }
    forEach(fn, thisArg) { for (const [k, v] of this.entries()) fn.call(thisArg, v, k, this); }
    [Symbol.iterator]() { return this.entries(); }
    get [Symbol.toStringTag]() { return 'Headers'; }
  }

  // Bodies are bytes; strings, URLSearchParams and JSON-ready values convert.
  const toBody = (body, headers) => {
    if (body === undefined || body === null) return null;
    if (typeof body === 'string') {
      if (!headers.has('content-type')) headers.set('content-type', 'text/plain;charset=UTF-8');
      return al.utf8Encode(body);
    }
    if (body instanceof URLSearchParams) {
      if (!headers.has('content-type')) headers.set('content-type', 'application/x-www-form-urlencoded;charset=UTF-8');
      return al.utf8Encode(body.toString());
    }
    if (body instanceof _Uint8Array) return new _Uint8Array(body);
    if (_isView(body)) return new _Uint8Array(bytesOf(body));
    if (body instanceof ArrayBuffer) return new _Uint8Array(body.slice(0));
    throw new TypeError('fetch: body must be a string, URLSearchParams, ArrayBuffer or typed array');
  };

  let bodyOf, setResponseMeta;
  class Body {
    #bytes;
    #used = false;
    static { bodyOf = (b) => b.#bytes; }
    constructor(bytes) { this.#bytes = bytes; }
    get bodyUsed() { return this.#used; }
    #take() {
      if (this.#used) throw new TypeError('Body has already been read');
      this.#used = true;
      return this.#bytes ?? new _Uint8Array(0);
    }
    async text() { return al.utf8Decode(this.#take(), false); }
    async json() { return JSON.parse(await this.text()); }
    async bytes() { return new _Uint8Array(this.#take()); }
    async arrayBuffer() { const b = this.#take(); return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength); }
  }

  class Request extends Body {
    constructor(input, init = {}) {
      const from = input instanceof Request ? input : null;
      const url = from ? from.url : new URL(String(input)).href;
      const headers = new Headers(init.headers ?? from?.headers);
      const method = String(init.method ?? from?.method ?? 'GET').toUpperCase();
      const body = init.body !== undefined ? toBody(init.body, headers) : from ? bodyOf(from) : null;
      if (body && (method === 'GET' || method === 'HEAD')) throw new TypeError(`fetch: a ${method} request can't have a body`);
      super(body);
      this.url = url;
      this.method = method;
      this.headers = headers;
      this.redirect = init.redirect ?? from?.redirect ?? 'follow';
    }
    clone() { return new Request(this); }
    get [Symbol.toStringTag]() { return 'Request'; }
  }

  class Response extends Body {
    #meta = { url: '', redirected: false };
    static { setResponseMeta = (r, meta) => { r.#meta = meta; }; }
    constructor(body = null, init = {}) {
      const headers = new Headers(init.headers);
      super(toBody(body, headers));
      this.status = init.status ?? 200;
      if (!Number.isInteger(this.status) || this.status < 200 || this.status > 599) throw new RangeError(`Response: status ${this.status} is out of range`);
      this.statusText = String(init.statusText ?? '');
      this.headers = headers;
      this.type = 'default';
    }
    get ok() { return this.status >= 200 && this.status < 300; }
    get url() { return this.#meta.url; }
    get redirected() { return this.#meta.redirected; }
    clone() {
      const r = new Response(bodyOf(this), { status: this.status, statusText: this.statusText, headers: this.headers });
      setResponseMeta(r, this.#meta);
      return r;
    }
    static json(data, init = {}) {
      const headers = new Headers(init.headers);
      if (!headers.has('content-type')) headers.set('content-type', 'application/json');
      return new Response(JSON.stringify(data), { ...init, headers });
    }
    get [Symbol.toStringTag]() { return 'Response'; }
  }

  async function fetch(input, init) {
    const req = new Request(input, init);
    const body = bodyOf(req);
    const res = await al.call('$fetch', [{ url: req.url, method: req.method, headers: [...req.headers], redirect: req.redirect, ...(body ? { body } : {}) }]);
    const out = new Response(res.body, { headers: res.headers });
    // The host's status may be one `new Response` refuses (a redirect, with redirect: 'manual').
    Object.defineProperty(out, 'status', { value: res.status, enumerable: true });
    out.statusText = res.statusText;
    out.type = 'basic';
    setResponseMeta(out, { url: res.url, redirected: res.redirected });
    return out;
  }
  hidden(g, 'Headers', Headers);
  hidden(g, 'Request', Request);
  hidden(g, 'Response', Response);
  hidden(g, 'queueMicrotask', (fn) => { Promise.resolve().then(fn); });

  // -- timers: backed by the host's $sleep call (real or virtual time) -------
  let timerSeq = 0;
  const timers = new _Map(); // timer id -> host call id
  const arm = (id, ms, fire) => {
    const p = al.call('$sleep', [Math.max(0, Number(ms) || 0)]);
    timers.set(id, al.lastCallId());
    p.then(fire);
  };
  hidden(g, 'setTimeout', (fn, ms = 0, ...args) => {
    if (typeof fn !== 'function') throw new TypeError('setTimeout: callback must be a function');
    const id = ++timerSeq;
    arm(id, ms, () => { if (timers.delete(id)) fn(...args); });
    return id;
  });
  hidden(g, 'setInterval', (fn, ms = 0, ...args) => {
    if (typeof fn !== 'function') throw new TypeError('setInterval: callback must be a function');
    const id = ++timerSeq;
    const tick = () => { if (timers.has(id)) { fn(...args); if (timers.has(id)) arm(id, Math.max(1, ms), tick); } };
    arm(id, Math.max(1, ms), tick);
    return id;
  });
  const clear = (id) => {
    const callId = timers.get(id);
    if (callId === undefined) return;
    timers.delete(id);
    al.cancel(callId);
  };
  hidden(g, 'clearTimeout', clear);
  hidden(g, 'clearInterval', clear);

  // -- host functions ---------------------------------------------------------
  // `install(["tools.search", "db.orders.list", "sh"])` builds namespaces of
  // async functions. Any depth works; every call becomes one host request.
  const installed = new _Set();
  function install(paths) {
    for (const path of paths) {
      if (installed.has(path)) continue;
      installed.add(path);
      // Host internals back built-ins rather than becoming globals; `fetch`
      // only exists when the host provides `$fetch`.
      if (path.startsWith('$')) {
        if (path === '$fetch') hidden(g, 'fetch', fetch);
        continue;
      }
      const parts = path.split('.');
      let obj = g;
      for (let i = 0; i < parts.length - 1; i++) {
        const key = parts[i];
        if (obj[key] === undefined || obj[key] === null) (i === 0 ? hidden : visible)(obj, key, {});
        obj = obj[key];
      }
      const name = parts[parts.length - 1];
      const fn = { [name]: (...args) => al.call(path, args) }[name];
      (parts.length === 1 ? hidden : visible)(obj, name, fn);
    }
  }

  // -- codec helpers used by the Rust side ------------------------------------
  const viewKinds = {
    Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array,
    Float32Array, Float64Array, BigInt64Array, BigUint64Array,
  };
  return {
    install,
    // The real contents, read with the original forEach: a replaced
    // Symbol.iterator (on the value or the prototype) doesn't change them.
    entries: (m) => { const out = []; _apply(_mapForEach, m, [(v, k) => push(out, [k, v])]); return out; },
    values: (s) => { const out = []; _apply(_setForEach, s, [(v) => push(out, v)]); return out; },
    mkMap: (entries) => new _Map(entries),
    mkSet: (values) => new _Set(values),
    mkRegExp: (src, flags) => new _RegExp(src, flags),
    mkBigInt: (s) => _BigInt(s),
    mkError: (name, message, stack, extra) => {
      const Ctor = errorTypes[name] || _Error;
      const e = new Ctor(message);
      if (!(name in errorTypes)) hidden(e, 'name', name);
      if (stack) hidden(e, 'stack', stack);
      if (extra) for (const k of Object.keys(extra)) e[k] = extra[k];
      return e;
    },
    viewBytes: (v) => bytesOf(v),
    mkView: (kind, bytes) => {
      const Ctor = viewKinds[kind];
      if (!Ctor) return bytes;
      const buf = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
      return new Ctor(buf);
    },
    encodeAs: ENCODE,
    ctorName: (v) => {
      try { const c = Object.getPrototypeOf(v)?.constructor; return typeof c === 'function' ? c.name : ''; } catch { return ''; }
    },
  };
})
