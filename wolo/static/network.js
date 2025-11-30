import morphdom from 'https://esm.run/morphdom';

/**
 * @param {HTMLElement} node 
 */
function clipboardCopy(node) {
    navigator.clipboard.writeText(node.textContent);
    node.classList.add('copied');

    if (node.copyTimeout) {
        clearTimeout(node.copyTimeout);
    }

    node.copyTimeout = setTimeout(function() {
        node.classList.remove('copied');
    }, 500);
}

/**
 * @param {HTMLElement} node 
 * @param {string} className 
 * @returns {HTMLElement|null}
 */
function findNextByClass(node, className) {
    let sibling = node.nextElementSibling;

    while (sibling) {
        if (sibling.classList.contains(className)) {
            return sibling;
        }

        sibling = sibling.nextElementSibling;
    }

    return null;
}

document.addEventListener('DOMContentLoaded', function() {
    let options = {
        onNodeAdded: function(node) {
            if (!(node instanceof HTMLElement)) {
                return;
            }

            if (node.classList.contains('copyable')) {
                let copy = findNextByClass(node, 'copy');

                if (copy) {
                    copy.addEventListener('click', function(e) {
                        console.log(node);
                        clipboardCopy(node);
                    });
                }
            }
        },
    };

    document.body.querySelectorAll('*').forEach(function(element) {
        options.onNodeAdded(element);
    });

    let value = parseInt(document.body.getAttribute('data-auto-refresh') || "0");

    if (!value) {
        return;
    }

    setInterval(function() {
        fetch(window.location.href).then(response => {
            if (response.status === 200) {
                return response.text();
            }
        }).then(html => {
            const parser = new DOMParser();
            const doc = parser.parseFromString(html, 'text/html');
            morphdom(document.body, doc.body, options);
        });
    }, value);
});
