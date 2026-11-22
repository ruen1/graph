(() => {
  'use strict';

  const eventRecords = new WeakMap();
  const unitless = new Set(['opacity', 'zIndex', 'fontWeight', 'lineHeight', 'zoom']);

  function nodes(value, context) {
    if (value == null) return [];
    if (value instanceof Dom) return value.elements;
    if (typeof value === 'string') {
      if (value.trim().startsWith('<')) {
        const template = document.createElement('template');
        template.innerHTML = value.trim();
        return Array.from(template.content.childNodes);
      }
      const scopes = context instanceof Dom ? context.elements : [context || document];
      return unique(scopes.flatMap(scope => Array.from(scope.querySelectorAll(value))));
    }
    if (value.nodeType || value === window || value === document) return [value];
    if (typeof value.length === 'number') return Array.from(value);
    return [value];
  }

  function unique(values) {
    return Array.from(new Set(values));
  }

  function contentNodes(value) {
    if (typeof value === 'string' && !value.trim().startsWith('<')) {
      return [document.createTextNode(value)];
    }
    return nodes(value);
  }

  class Dom {
    constructor(elements) {
      this.elements = elements;
      this.length = elements.length;
      elements.forEach((element, index) => {
        this[index] = element;
      });
    }

    get(index) {
      return index == null ? this.elements.slice() : this.elements[index < 0 ? this.length + index : index];
    }

    map(callback) {
      const values = [];
      this.elements.forEach((element, index) => {
        const value = callback.call(element, index, element);
        if (value != null) values.push(...(Array.isArray(value) ? value : [value]));
      });
      return new Dom(values);
    }

    find(selector) {
      return new Dom(unique(this.elements.flatMap(element => Array.from(element.querySelectorAll(selector)))));
    }

    getElementValue(property, value) {
      if (value === undefined) return this.elements[0]?.[property];
      this.elements.forEach(element => {
        element[property] = value;
      });
      return this;
    }

    attr(name, value) {
      if (value === undefined) return this.elements[0]?.getAttribute(name) ?? undefined;
      this.elements.forEach(element => element.setAttribute(name, value));
      return this;
    }

    removeAttr(name) {
      this.elements.forEach(element => element.removeAttribute(name));
      return this;
    }

    val(value) {
      return this.getElementValue('value', value);
    }

    text(value) {
      if (value === undefined) return this.elements.map(element => element.textContent).join('');
      this.elements.forEach(element => {
        element.textContent = value;
      });
      return this;
    }

    html(value) {
      return this.getElementValue('innerHTML', value);
    }

    empty() {
      this.elements.forEach(element => element.replaceChildren());
      return this;
    }

    append(value) {
      const additions = contentNodes(value);
      this.elements.forEach((element, index) => {
        additions.forEach(child => element.appendChild(index ? child.cloneNode(true) : child));
      });
      return this;
    }

    appendTo(target) {
      const destinations = nodes(target);
      const inserted = [];
      destinations.forEach((destination, index) => {
        this.elements.forEach(element => {
          const child = index ? element.cloneNode(true) : element;
          destination.appendChild(child);
          inserted.push(child);
        });
      });
      return new Dom(inserted);
    }

    remove() {
      this.elements.forEach(element => element.remove());
      return this;
    }

    clone() {
      return new Dom(this.elements.map(element => element.cloneNode(true)));
    }

    replaceWith(value) {
      const replacements = contentNodes(value);
      this.elements.forEach((element, index) => {
        const fragment = document.createDocumentFragment();
        replacements.forEach(child => fragment.appendChild(index ? child.cloneNode(true) : child));
        element.replaceWith(fragment);
      });
      return this;
    }

    css(name, value) {
      const properties = typeof name === 'object' ? name : { [name]: value };
      Object.entries(properties).forEach(([property, item]) => {
        const cssValue = typeof item === 'number' && !unitless.has(property) ? `${item}px` : item;
        this.elements.forEach(element => {
          element.style[property] = cssValue;
        });
      });
      return this;
    }

    show() {
      this.elements.forEach(element => {
        element.style.display = '';
        if (getComputedStyle(element).display === 'none') element.style.display = 'block';
      });
      return this;
    }

    offset() {
      const element = this.elements[0];
      if (!element) return undefined;
      const bounds = element.getBoundingClientRect();
      return {
        left: bounds.left + window.scrollX,
        top: bounds.top + window.scrollY,
      };
    }

    outerWidth() {
      const element = this.elements[0];
      if (element === window) return window.innerWidth;
      return element?.getBoundingClientRect().width || 0;
    }

    outerHeight() {
      const element = this.elements[0];
      if (element === window) return window.innerHeight;
      return element?.getBoundingClientRect().height || 0;
    }

    on(types, handler) {
      types.split(/\s+/).filter(Boolean).forEach(type => {
        this.elements.forEach(element => {
          element.addEventListener(type, handler);
          const records = eventRecords.get(element) || [];
          records.push({ type, handler });
          eventRecords.set(element, records);
        });
      });
      return this;
    }

    off(types) {
      const wanted = types.split(/\s+/).filter(Boolean);
      this.elements.forEach(element => {
        const remaining = [];
        (eventRecords.get(element) || []).forEach(record => {
          const remove = wanted.includes(record.type);
          if (remove) element.removeEventListener(record.type, record.handler);
          else remaining.push(record);
        });
        eventRecords.set(element, remaining);
      });
      return this;
    }

    focus() {
      this.elements[0]?.focus();
      return this;
    }

    selection(action, position) {
      const element = this.elements[0];
      if (!element) return action === 'getPos' ? { start: 0, end: 0 } : this;
      if (action === 'getPos') {
        return { start: element.selectionStart, end: element.selectionEnd };
      }
      if (action === 'setPos') {
        element.setSelectionRange(position.start, position.end);
      }
      return this;
    }
  }

  for (const type of ['click', 'keydown', 'change', 'submit', 'mouseover', 'mouseout', 'blur']) {
    Dom.prototype[type] = function (handler) {
      return this.on(type, handler);
    };
  }

  function $(value, context) {
    return new Dom(nodes(value, context));
  }

  $.ajax = function (url, options) {
    if (typeof url === 'object') {
      options = url;
      url = options.url;
    }
    const xhr = options.xhr ? options.xhr() : new XMLHttpRequest();
    const method = options.type.toUpperCase();
    xhr.open(method, url, true);
    if (options.xhrFields?.withCredentials) xhr.withCredentials = true;
    if (options.contentType !== false) {
      xhr.setRequestHeader('Content-Type', options.contentType || 'application/x-www-form-urlencoded; charset=UTF-8');
    }

    let completed = false;
    xhr.addEventListener('readystatechange', () => {
      if (xhr.readyState !== XMLHttpRequest.DONE || completed) return;
      completed = true;
      let response = xhr.responseText;
      if (options.dataType === 'json') {
        try {
          response = JSON.parse(response);
        } catch (error) {
          options.error?.(xhr, 'parsererror', error);
          return;
        }
      }
      if (xhr.status >= 200 && xhr.status < 300 || xhr.status === 304) options.success?.(response, 'success', xhr);
      else options.error?.(xhr, 'error', xhr.statusText);
    });
    xhr.addEventListener('error', () => options.error?.(xhr, 'error', xhr.statusText));
    if (options.beforeSend?.(xhr, options) === false) return xhr;
    xhr.send(options.data ?? null);
    return xhr;
  };

  function autosize(element) {
    element.style.overflowY = 'hidden';
    autosize.update(element);
  }

  autosize.update = function (element) {
    element.style.height = 'auto';
    element.style.height = `${element.scrollHeight}px`;
  };

  window.$ = $;
  window.autosize = autosize;
})();
