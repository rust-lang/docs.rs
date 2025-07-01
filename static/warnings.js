(function() {
    function load(key, def = null) {
        const value = window.localStorage.getItem(key);
        if (value) {
            try {
                return JSON.parse(value);
            } catch (ex) {
                console.error(`Failed loading ${key} from local storage`, ex);
                return def;
            }
        } else {
            return def;
        }
    }

    function store(key, value) {
        window.localStorage.setItem(key, JSON.stringify(value));
    }

    function create(tagName, attrs = {}, children = [], listeners = {}) {
        const el = document.createElement(tagName);
        for (const key of Object.keys(attrs)) {
            if (typeof attrs[key] === "object") {
                for (const subkey of Object.keys(attrs[key])) {
                    el[key].setProperty(subkey, attrs[key][subkey]);
                }
            } else {
                el.setAttribute(key, attrs[key]);
            }
        }
        el.append(...children);
        for (const key of Object.keys(listeners)) {
            el.addEventListener(key, listeners[key]);
        }
        return el;
    }


    if (!load("docs-rs-warnings-enabled", false)) {
        return;
    }

    const parentEl = document.getElementById("warnings-menu-parent");
    parentEl.removeAttribute("hidden");

    const current = JSON.parse(document.getElementById("crate-metadata")?.innerText || null);

    const menuEl = document.getElementById("warnings-menu");

    const followed = load("docs-rs-warnings-followed", []);

    function update() {
        const children = [];

        if (followed.length > 0) {
            children.push(
                create("div", { class: "pure-g" }, [
                    create("div", { class: "pure-u-1" }, [
                        create("ul", { class: "pure-menu-list", style: { width: "100%" } }, [
                            create("li", { class: "pure-menu-heading" }, [
                                create("b", {}, ["Followed crates"]),
                            ]),
                            ...followed.map(name => (
                                create("li", { class: "pure-menu-item followed" }, [
                                    create("a", { class: "pure-menu-link", href: `/${name}` }, [
                                        name,
                                    ]),
                                    create(
                                        "a",
                                        { class: "pure-menu-link remove", href: "#" },
                                        ["🗙"],
                                        {
                                            click: _ => {
                                                const index = followed.indexOf(name);
                                                followed.splice(index, 1);
                                                store("docs-rs-warnings-followed", followed);
                                                update();
                                            },
                                        },
                                    ),
                                ])
                            )),
                        ]),
                    ]),
                ]),
            );
        }

        if (current && !followed.includes(current.name)) {
            children.push(
                create("div", { class: "pure-g" }, [
                    create("div", { class: "pure-u-1" }, [
                        create("ul", { class: "pure-menu-list", style: { width: "100%" } }, [
                            create("li", { class: "pure-menu-item" }, [
                                create("a", { class: "pure-menu-link", href: "#" }, [
                                    "Follow ",
                                    create("b", {}, [current.name]),
                                ], {
                                    click: () => {
                                        const i = followed.findIndex(name => name > current.name);
                                        if (i >= 0) {
                                            followed.splice(i, 0, current.name);
                                        } else {
                                            followed.push(current.name);
                                        }
                                        store("docs-rs-warnings-followed", followed);
                                        update();
                                    },
                                }),
                            ]),
                        ]),
                    ]),
                ]),
            );
        }

        for (const child of children.slice(0, -1)) {
            child.classList.add("menu-item-divided");
        }

        menuEl.replaceChildren(...children);
    }

    update();
})();
