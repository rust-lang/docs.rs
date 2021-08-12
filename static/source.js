(function() {
    var oldLabel;

    function showSourceFiles(button, sideMenu, sourceCode) {
        button.title = oldLabel;
        button.setAttribute("aria-label", button.title);
        button.setAttribute("aria-expanded", "true");

        sideMenu.classList.remove("collapsed");
        sourceCode.classList.remove("expanded");
    }

    function hideSourceFiles(button, sideMenu, sourceCode) {
        button.title = "Show source sidebar";
        button.setAttribute("aria-label", button.title);
        button.setAttribute("aria-expanded", "false");

        sideMenu.classList.add("collapsed");
        sourceCode.classList.add("expanded");
    }

    function toggleSource(button) {
        var sideMenu = document.getElementById("side-menu");
        var sourceCode = document.getElementById("source-code");

        if (sideMenu.classList.contains("collapsed")) {
            showSourceFiles(button, sideMenu, sourceCode);
        } else {
            hideSourceFiles(button, sideMenu, sourceCode);
        }
    }

    document.addEventListener("DOMContentLoaded", function(event) { 
        var toggleSourceButton = document.querySelector("li.toggle-source button");
        oldLabel = toggleSourceButton.getAttribute("aria-label");

        toggleSourceButton.addEventListener("click", function() {
            toggleSource(toggleSourceButton);
        });
    });
})();
