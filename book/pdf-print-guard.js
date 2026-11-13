// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// The PDF renderer starts its own print job after loading this page.
// REFERENCE: https://github.com/HollowMan6/mdbook-pdf/issues/78
if (navigator.webdriver && window.location.pathname.endsWith("/print.html")) {
    window.print = () => {};
}
