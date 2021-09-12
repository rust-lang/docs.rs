(function() {
    function focusSearchInput() {
        // On the index page, we have a "#search" input. If we are on this page, we want to go back
        // to this one and not the one in the header navbar.
        var searchInput = document.getElementById("search");
        if (searchInput) {
            searchInput.focus();
        } else {
            document.getElementById("nav-search").focus()
        }
    }

    function focusFirstSearchResult() {
        var elem = document.querySelector(".recent-releases-container a.release");
        if (elem) {
            elem.focus();
        }
    }

    function getWrappingLi(elem) {
        while (elem.tagName !== "LI") {
            elem = elem.parentElement;
        }
        return elem;
    }

    function focusOnLi(li) {
        var elem = li.querySelector(".release");
        if (elem) {
            elem.focus();
        }
    }

    function getKey(ev) {
        if ("key" in ev && typeof ev.key != "undefined") {
            return ev.key;
        }
        return String.fromCharCode(ev.charCode || ev.keyCode);
    }

    function checkIfHasParent(elem, className) {
        while (elem && elem.tagName !== "BODY") {
            elem = elem.parentElement;
            if (elem.classList.constains(className)) {
                return true;
            }
        }
        return false;
    }

    function handleKey(ev) {
        if (ev.ctrlKey || ev.altKey || ev.metaKey) {
            return;
        }
        var tagName = document.activeElement.tagName;
        if (["BODY", "INPUT"].indexOf(tagName) === -1 &&
            tagName !== "A" &&
            !checkIfHasParent(document.activeElement, "recent-releases-container"))
        {
            return;
        }

        if (ev.which === 40) { // Down arrow
            ev.preventDefault();
            if (tagName === "BODY") {
                focusFirstSearchResult();
            } else {
                var wrappingLi = getWrappingLi(document.activeElement);
                if (wrappingLi.nextElementSibling) {
                    focusOnLi(wrappingLi.nextElementSibling);
                }
            }
        } else if (ev.which === 38) { // Up arrow
            ev.preventDefault();
            if (tagName === "A") {
                var wrappingLi = getWrappingLi(document.activeElement);
                if (wrappingLi.previousElementSibling)
                {
                    focusOnLi(wrappingLi.previousElementSibling);
                } else {
                    focusSearchInput();
                }
            } else if (tagName === "BODY") {
                focusFirstSearchResult();
            }
        } else if (ev.which === 27) { // Escape
            document.activeElement.blur();
        } else if (tagName !== "INPUT") {
            switch (getKey(ev)) {
                case "s":
                case "S":
                    ev.preventDefault();
                    focusSearchInput();
                    break;
            }
        }
    }

    document.onkeypress = handleKey;
    document.onkeydown = handleKey;
})();
