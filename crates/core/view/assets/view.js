// Shared HTMX configuration for hypgm-view pages. Load after htmx.min.js.
//
// Form section convention: a domain wraps each enhanced form and its results
// in an element marked `data-view-section`. The form posts to its ordinary
// action with `hx-target="closest [data-view-section]"` and
// `hx-swap="outerHTML"`, and the section contains a `data-view-feedback`
// element for request failures. Fragment responses render the same section.
(function () {
  "use strict";

  if (typeof htmx === "undefined") {
    return;
  }

  var GENERIC_FAILURE =
    "The request could not be completed. Check your connection and try again.";

  // History restores fetch full pages without HX-Request so servers never
  // return a fragment for a whole-document replacement.
  htmx.config.historyRestoreAsHxRequest = false;

  // Controlled HTML 4xx/5xx responses are swapped; HTTP statuses are not
  // rewritten, and htmx still reports them as errors.
  htmx.config.responseHandling = [
    { code: "204", swap: false },
    { code: "[23]..", swap: true },
    { code: "[45]..", swap: true, error: true },
  ];

  function sectionOf(element) {
    if (element && element.closest) {
      return element.closest("[data-view-section]");
    }
    return null;
  }

  function feedbackOf(element) {
    var section = sectionOf(element);
    return (section || document).querySelector("[data-view-feedback]");
  }

  function showFeedback(element, message) {
    var feedback = feedbackOf(element);
    if (feedback) {
      feedback.textContent = message;
      feedback.hidden = false;
    }
  }

  function clearFeedback(element) {
    var feedback = feedbackOf(element);
    if (feedback) {
      feedback.textContent = "";
      feedback.hidden = true;
    }
  }

  function isHtml(xhr) {
    var type = (xhr.getResponseHeader("Content-Type") || "").toLowerCase();
    return type.indexOf("text/html") === 0;
  }

  document.addEventListener("htmx:beforeRequest", function (event) {
    clearFeedback(event.target);
  });

  document.addEventListener("htmx:beforeSwap", function (event) {
    var detail = event.detail;
    if (!detail.isError) {
      return;
    }
    var xhr = detail.xhr;
    if (isHtml(xhr)) {
      var doc = new DOMParser().parseFromString(xhr.responseText, "text/html");
      // Domain-rendered sections carry their own escaped validation state.
      if (doc.querySelector("[data-view-section]")) {
        detail.shouldSwap = true;
        return;
      }
      // Shared HTML errors keep the form in place and show their safe message.
      var error = doc.querySelector("[data-view-error]");
      detail.shouldSwap = false;
      showFeedback(event.target, error ? error.textContent : GENERIC_FAILURE);
      return;
    }
    detail.shouldSwap = false;
    showFeedback(event.target, GENERIC_FAILURE);
  });

  function networkFailure(event) {
    showFeedback(event.target, GENERIC_FAILURE);
  }

  document.addEventListener("htmx:sendError", networkFailure);
  document.addEventListener("htmx:timeout", networkFailure);
})();
