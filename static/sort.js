function populateByName(source, target) {
	let posts = [];
	for (let post of source.children) {
		let title = post.firstElementChild.innerText;
		posts.push([title, post.cloneNode(true)]);
	}
	posts.sort(([a, _1], [b, _2]) => a.toLocaleLowerCase().localeCompare(b.toLocaleLowerCase()));
	for (let [_, post] of posts) {
		target.appendChild(post);
	}
}

function sort(by, dateEl, nameEl) {
	console.log("sorting by", by);
	switch (by) {
		case "date":
			dateEl.style.display = "block";
			nameEl.style.display = "none";
			break;
		case "name":
			nameEl.style.display = "block";
			dateEl.style.display = "none";
			break;
	}
}

function handleSort(form, dateEl, nameEl) {
	for (let el of form.sort)
		el.addEventListener("change", () => {
			if (el.checked) sort(el.value, dateEl, nameEl);
		});
}
