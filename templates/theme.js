// This is a global function also called from a script in ./rustdoc/body.html
// which detects when the rustdoc theme is changed
function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
}

applyTheme(window.localStorage.getItem('rustdoc-theme'));
