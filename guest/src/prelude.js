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
    ctorName: (v) => {
      try { const c = Object.getPrototypeOf(v)?.constructor; return typeof c === 'function' ? c.name : ''; } catch { return ''; }
    },
  };
})
