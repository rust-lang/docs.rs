(function() {
    function getKey(ev) {
        if ("key" in ev && typeof ev.key != "undefined") {
            return ev.key;
        }
        return String.fromCharCode(ev.charCode || ev.keyCode);
    }

    var active = null;
    function handleKey(ev) {
        if (ev.ctrlKey || ev.altKey || ev.metaKey || document.activeElement.tagName === "INPUT") {
            return;
        }

        if (ev.which === 40) { // Down arrow
            ev.preventDefault();
            if (active === null) {
                active = document.getElementsByClassName("recent-releases-container")[0].getElementsByTagName("li")[0];
            } else if (active.nextElementSibling) {
                active.classList.remove("selected");
                active = active.nextElementSibling;
            }
            active.classList.add("selected");
        } else if (ev.which === 38) { // Up arrow
            ev.preventDefault();
            if (active === null) {
                active = document.getElementsByClassName("recent-releases-container")[0].getElementsByTagName("li")[0];
            } else if (active.previousElementSibling) {
                active.classList.remove("selected");
                active = active.previousElementSibling;
            }
            active.classList.add("selected");
            active.focus();
        } else if (ev.which === 13) { // Return
            if (active !== null) {
                document.location.href = active.getElementsByTagName("a")[0].href;
            }
        } else {
            switch (getKey(ev)) {
                case "s":
                case "S":
                    ev.preventDefault();
                    var searchInputNav = document.getElementsByClassName("search-input-nav");
                    if (searchInputNav.length > 0) {
                        searchInputNav[0].focus();
                    }
                    break;
            }
        }
    }

    document.onkeypress = handleKey;
    document.onkeydown = handleKey;

    var crates = Array.prototype.slice.call(document.getElementsByClassName("recent-releases-container")[0].getElementsByTagName("li"));
    for (var i = 0; i < crates.length; ++i) {
        crates[i].addEventListener("mouseover", function (event) {
            this.classList.remove("selected");
            active = null;
        });
        crates[i].addEventListener("mouseout", function (event) {
            this.classList.remove("selected");
            active = null;
        });
    }
})();
