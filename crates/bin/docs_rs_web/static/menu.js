// Allow menus to be opened and used by keyboard.
(function() {
    const updateMenuPositionForSubMenu = currentMenuSupplier => {
        const currentMenu = currentMenuSupplier();
        const subMenu = currentMenu?.getElementsByClassName("pure-menu-children")?.[0];

        subMenu?.style.setProperty("--menu-x", `${currentMenu.getBoundingClientRect().x}px`);
    };

    const loadedMenus = new Set();

    async function loadAjaxMenu(menu, id, msg) {
        if (loadedMenus.has(id)) {
            return;
        }
        loadedMenus.add(id);
        if (!menu.querySelector(".rotate")) {
            return;
        }
        const listElem = document.getElementById(id);
        if (!listElem) {
            // We're not in a documentation page, so no need to do anything.
            return;
        }
        const url = listElem.dataset.url;
        try {
            const response = await fetch(url);
            listElem.innerHTML = await response.text();
        } catch (ex) {
            console.error(`Failed to load ${msg}: ${ex}`);
            listElem.innerHTML = `Failed to load ${msg}`;
        }
    }

    let currentMenu;
    const backdrop = document.createElement("div");
    backdrop.style = "display:none;position:fixed;width:100%;height:100%;z-index:1";
    document.documentElement.insertBefore(backdrop, document.querySelector("body"));

    addEventListener("resize", () => updateMenuPositionForSubMenu(() => currentMenu));

    function previous(allItems, item) {
        let i = 1;
        const l = allItems.length;
        while (i < l) {
            if (allItems[i] === item) {
                return allItems[i - 1];
            }
            i += 1;
        }
    }
    function next(allItems, item) {
        let i = 0;
        const l = allItems.length - 1;
        while (i < l) {
            if (allItems[i] === item) {
                return allItems[i + 1];
            }
            i += 1;
        }
    }
    function last(allItems) {
        return allItems[allItems.length - 1];
    }
    function closeMenu() {
        if (this === backdrop) {
            document.documentElement.focus();
        } else if (currentMenu.querySelector(".pure-menu-link:focus")) {
            currentMenu.firstElementChild.focus();
        }
        currentMenu.classList.remove("pure-menu-active");
        currentMenu = null;
        backdrop.style.display = "none";
    }
    backdrop.onclick = closeMenu;
    function openMenu(newMenu) {
        updateMenuPositionForSubMenu(() => newMenu);
        currentMenu = newMenu;
        newMenu.classList.add("pure-menu-active");
        backdrop.style.display = "block";

        if (newMenu.querySelector("#releases-list")) {
            loadAjaxMenu(
                newMenu,
                "releases-list",
                "release list",
            );

        } else if (newMenu.querySelector("#platforms")) {
            loadAjaxMenu(
                newMenu,
                "platforms",
                "platforms list",
            );
        }
    }
    function menuOnClick(e) {
        if (this.getAttribute("href") !== "#") {
            return;
        }
        if (this.parentNode === currentMenu) {
            closeMenu();
            this.blur();
        } else {
            if (currentMenu) closeMenu();

            openMenu(this.parentNode);
        }
        e.preventDefault();
        e.stopPropagation();
    }
    function menuKeyDown(e) {
        if (currentMenu) {
            const children = currentMenu.querySelector(".pure-menu-children");
            const currentLink = children.querySelector(".pure-menu-link:focus");
            let currentItem;
            if (currentLink && currentLink.parentNode.classList.contains("pure-menu-item")) {
                currentItem = currentLink.parentNode;
            }
            let allItems = [];
            if (children) {
                allItems = children.querySelectorAll(".pure-menu-item .pure-menu-link");
            }
            let switchTo = null;
            switch (e.key.toLowerCase()) {
                case "escape":
                case "esc":
                    closeMenu();
                    e.preventDefault();
                    e.stopPropagation();
                    return;
                case "arrowdown":
                case "down":
                    if (currentLink) {
                        // Arrow down when an item other than the last is focused:
                        //      focus next item.
                        // Arrow down when the last item is focused:
                        //      jump to top.
                        switchTo = (next(allItems, currentLink) || allItems[0]);
                    } else {
                        // Arrow down when a menu is open and nothing is focused:
                        //      focus first item.
                        switchTo = allItems[0];
                    }
                    break;
                case "arrowup":
                case "up":
                    if (currentLink) {
                        // Arrow up when an item other than the first is focused:
                        //      focus previous item.
                        // Arrow up when the first item is focused:
                        //      jump to bottom.
                        switchTo = (previous(allItems, currentLink) || last(allItems));
                    } else {
                        // Arrow up when a menu is open and nothing is focused: focus last item.
                        switchTo = last(allItems);
                    }
                    break;
                case "tab":
                    if (!currentLink) {
                        // if the menu is open, we should focus trap into it
                        //
                        // this is the behavior of the WAI example, it is not the same as GitHub,
                        // but GitHub allows you to tab yourself out of the menu without closing it
                        // (which is horrible behavior)
                        switchTo = e.shiftKey ? last(allItems) : allItems[0];
                    } else if (e.shiftKey && currentLink === allItems[0]) {
                        // if you tab your way out of the menu, close it
                        //
                        // this is neither what GitHub nor the WAI example do, but is a
                        // rationalization of GitHub's behavior: we don't want users who know how to
                        // use tab and enter, but don't know that they can close menus with Escape,
                        // to find themselves completely trapped in the menu
                        closeMenu();
                        e.preventDefault();
                        e.stopPropagation();
                    } else if (!e.shiftKey && currentLink === last(allItems)) {
                        // same as above.
                        // if you tab your way out of the menu, close it
                        closeMenu();
                    }
                    break;
                case "enter":
                case "return":
                    // enter and return have the default browser behavior,
                    // but they also close the menu
                    // this behavior is identical between both the WAI example, and GitHub's
                    setTimeout(() => {
                        closeMenu();
                    }, 100);
                    break;
                case "space":
                case " ": {
                    // space closes the menu, and activates the current link
                    // this behavior is identical between both the WAI example, and GitHub's
                    const hasPopup = document.activeElement instanceof HTMLAnchorElement
                        && !document.activeElement.hasAttribute("aria-haspopup");
                    if (hasPopup) {
                        // It's supposed to copy the behaviour of the WAI Menu Bar
                        // page, and of GitHub's menus. I've been using these two
                        // sources to judge what is basically "industry standard"
                        // behaviour for menu keyboard activity on the web.
                        //
                        // On GitHub, here's what I notice:
                        //
                        // 1 If you click open a menu, the menu button remains
                        //   focused. If, in this stage, I press space, the menu will
                        //   close.
                        //
                        // 2 If I use the arrow keys to focus a menu item, and then
                        //   press space, the menu item will be activated. For
                        //   example, clicking "+", then pressing down, then pressing
                        //   space will open the New Repository page.
                        //
                        // Behaviour 1 is why the
                        // `!document.activeElement.hasAttribute("aria-haspopup")`
                        // condition is there. It's to make sure the menu-link on
                        // things like the About dropdown don't get activated.
                        // Behaviour 2 is why this code is required at all; I want to
                        // activate the currently highlighted menu item.
                        document.activeElement.click();
                    }
                    setTimeout(() => {
                        closeMenu();
                    }, 100);
                    e.preventDefault();
                    e.stopPropagation();
                    break;
                }
                case "home":
                    // home: focus first menu item.
                    //
                    // This is the behavior of WAI, while GitHub scrolls, but it's unlikely that a
                    // user will try to scroll the page while the menu is open, so they won't do it
                    // on accident.
                    switchTo = allItems[0];
                    break;
                case "end":
                    // end: focus last menu item.
                    //
                    // This is the behavior of WAI, while GitHub scrolls, but it's unlikely that a
                    // user will try to scroll the page while the menu is open, so they won't do it
                    // on accident.
                    switchTo = last(allItems);
                    break;
                case "pageup":
                    // page up: jump five items up, stopping at the top
                    //
                    // the number 5 is used so that we go one page in the inner-scrolled
                    // Dependencies and Versions fields
                    switchTo = currentItem || allItems[0];
                    for (let n = 0; n < 5; ++n) {
                        const hasPrevious = switchTo.previousElementSibling
                            && switchTo.previousElementSibling.classList.contains("pure-menu-item");
                        if (hasPrevious) {
                            switchTo = switchTo.previousElementSibling;
                        }
                    }
                    break;
                case "pagedown":
                    // page down: jump five items down, stopping at the bottom
                    // the number 5 is used so that we go one page in the
                    // inner-scrolled Dependencies and Versions fields
                    switchTo = currentItem || last(allItems);
                    for (let n = 0; n < 5; ++n) {
                        const hasNext = switchTo.nextElementSibling
                            && switchTo.nextElementSibling.classList.contains("pure-menu-item");
                        if (hasNext) {
                            switchTo = switchTo.nextElementSibling;
                        }
                    }
                    break;
            }
            if (switchTo) {
                const switchToLink = switchTo.querySelector("a");
                if (switchToLink) {
                    switchToLink.focus();
                } else {
                    switchTo.focus();
                }
                e.preventDefault();
                e.stopPropagation();
            }
        } else if (e.target.parentNode &&
            e.target.parentNode.classList &&
            e.target.parentNode.classList.contains("pure-menu-has-children")
        ) {
            switch (e.key.toLowerCase()) {
                case "arrowdown":
                case "down":
                case "space":
                case " ":
                    openMenu(e.target.parentNode);
                    e.preventDefault();
                    e.stopPropagation();
                    break;
            }
        }
    }
    for (const menu of document.querySelectorAll(".pure-menu-has-children")) {
        menu.firstElementChild.setAttribute("aria-haspopup", "menu");
        menu.firstElementChild.nextElementSibling.setAttribute("role", "menu");
        menu.firstElementChild.addEventListener("click", menuOnClick);
    }
    document.documentElement.addEventListener("keydown", menuKeyDown);
    document.documentElement.addEventListener("keydown", ev => {
        if (ev.key === "y" && ev.target.tagName !== "INPUT") {
            const permalink = document.getElementById("permalink");
            if (document.location.hash !== "") {
              permalink.href += document.location.hash;
            }
            history.replaceState({}, null, permalink.href);
        }
    });
})();
