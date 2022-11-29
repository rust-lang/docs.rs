// Improve interactions with dropdown menus.
(function() {
    const OPEN_MENU_SELECTOR = ".nav-container details[open]";

    function updateMenuPositionForSubMenu() {
        const currentMenu = document.querySelector(OPEN_MENU_SELECTOR);
        const subMenu = currentMenu?.getElementsByClassName('pure-menu-children')?.[0];

        subMenu?.style.setProperty('--menu-x', `${currentMenu.getBoundingClientRect().x}px`);
    }

    addEventListener('resize', updateMenuPositionForSubMenu);

    function previous(allItems, item) {
        var i = 1;
        var l = allItems.length;
        while (i < l) {
            if (allItems[i] == item) {
                return allItems[i - 1];
            }
            i += 1;
        }
    }
    function next(allItems, item) {
        var i = 0;
        var l = allItems.length - 1;
        while (i < l) {
            if (allItems[i] == item) {
                return allItems[i + 1];
            }
            i += 1;
        }
    }
    function last(allItems) {
        return allItems[allItems.length - 1];
    }
    function closeMenu(ignore) {
        const menus = Array.prototype.slice.call(
            document.querySelectorAll(OPEN_MENU_SELECTOR));
        for (const menu of menus) {
            if (menu !== ignore) {
                menu.open = false;
            }
        }
    }
    function menuOnClick(e) {
        if (!this.open) {
            this.focus();
        } else {
            closeMenu(this);
            updateMenuPositionForSubMenu();
        }
    };
    function menuKeyDown(e) {
        const key = e.key.toLowerCase();
        if ((key === "escape" || key === "esc") &&
            document.querySelector(OPEN_MENU_SELECTOR) !== null)
        {
            closeMenu();
            e.preventDefault();
            e.stopPropagation();
        }
    }

    const setEvents = (menus) => {
        menus = Array.prototype.slice.call(menus);
        for (const menu of menus) {
            menu.addEventListener("toggle", menuOnClick);
            menu.addEventListener("keydown", menuKeyDown);
        }
    };
    setEvents(document.querySelectorAll(".nav-container details"));

    document.documentElement.addEventListener("keydown", function(ev) {
        if (ev.key == "y" && ev.target.tagName != "INPUT") {
            let permalink = document.getElementById("permalink");
            if (document.location.hash != "") {
              permalink.href += document.location.hash;
            }
            history.replaceState({}, null, permalink.href);
        }
    });
})();
