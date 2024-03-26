(function() {
    function focusSearchInput() {
        // On the index page, we have a "#search" input. If we are on this page, we want to go back
        // to this one and not the one in the header navbar.
        const searchInput = document.getElementById("search");
        if (searchInput) {
            searchInput.focus();
        } else {
            document.getElementById("nav-search").focus();
        }
    }

    function focusFirstSearchResult() {
        const elem = document.querySelector(".recent-releases-container a.release");
        if (elem) {
            elem.focus();
        }
    }

    function getWrappingLi(elem) {
        while (elem && elem.tagName !== "LI" && elem.tagName !== "BODY") {
            elem = elem.parentElement;
        }
        if (elem.tagName === "BODY") {
            return null;
        }
        return elem;
    }

    function focusOnLi(li) {
        const elem = li.querySelector(".release");
        if (elem) {
            elem.focus();
        }
    }

    function getKey(ev) {
        if ("key" in ev && typeof ev.key !== "undefined") {
            return ev.key;
        }
        return String.fromCharCode(ev.charCode || ev.keyCode);
    }

    function checkIfHasParent(elem, className) {
        while (elem && elem.tagName !== "BODY") {
            elem = elem.parentElement;
            if (elem.classList.contains(className)) {
                return true;
            }
        }
        return false;
    }

    function handleKey(ev) {
        if (ev.ctrlKey || ev.altKey || ev.metaKey) {
            return;
        }
        if (ev.which === 27) { // Escape
            document.activeElement.blur();
            return;
        }
        const tagName = document.activeElement.tagName;
        // We want to check two things here: if an input or an element of the docs.rs topbar
        // has the focus. If so, then we do nothing and simply return.
        if (tagName === "INPUT" ||
            (tagName === "A" &&
                checkIfHasParent(document.activeElement, "nav-container"))) {
            return;
        }

        if (ev.which === 40) { // Down arrow
            ev.preventDefault();
            if (tagName === "BODY") {
                focusFirstSearchResult();
            } else {
                const wrappingLi = getWrappingLi(document.activeElement);
                if (!wrappingLi) {
                    // Doesn't seem like we are in the crates list, let's focus the first element
                    // of the list then!
                    focusFirstSearchResult();
                } else if (wrappingLi.nextElementSibling) {
                    focusOnLi(wrappingLi.nextElementSibling);
                }
            }
        } else if (ev.which === 38) { // Up arrow
            ev.preventDefault();
            if (tagName === "A") {
                const wrappingLi = getWrappingLi(document.activeElement);
                if (!wrappingLi) {
                    // Doesn't seem like we are in the crates list, let's focus the first element
                    // of the list then!
                    focusFirstSearchResult();
                } else if (wrappingLi.previousElementSibling) {
                    focusOnLi(wrappingLi.previousElementSibling);
                } else {
                    focusSearchInput();
                }
            } else if (tagName === "BODY") {
                focusFirstSearchResult();
            }
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

    function handleSortByChange() {
        const inputSearch = document.getElementById("nav-search");
        const searchForm = document.getElementById("nav-search-form");
        if (inputSearch.value && searchForm) {
            searchForm.submit();
        }
    }
    const searchSortBySel = document.getElementById("nav-sort");
    if (searchSortBySel) {
        searchSortBySel.addEventListener("change", handleSortByChange);
    }

    document.onkeypress = handleKey;
    document.onkeydown = handleKey;
})();
