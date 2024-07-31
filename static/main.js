let form = document.getElementById("sort");
let posts = document.getElementById("posts");

let postsName = document.createElement("div");

function initialSort(source, target) {
	let posts = [];
	for (let post of source.children) {
		let title = post.firstElementChild.innerText;
		posts.push([title, post.cloneNode(true)]);
	}
	posts.sort(([a, _1], [b, _2]) => a.toLocaleLowerCase() > b.toLocaleLowerCase());
	for (let [_, post] of posts) {
		target.appendChild(post);
	}
}

function sort(by) {
	console.log("sorting by", by);
	switch (by) {
		case "date":
			posts.style.display = "block";
			postsName.style.display = "none";
			break;
		case "name":
			postsName.style.display = "block";
			posts.style.display = "none";
			break;
	}
}

function handleSort() {
	if (!form) return;
	for (let el of form.sort) el.addEventListener("change", () => sort(form.sort.value));
}

initialSort(posts, postsName);
posts.parentNode.appendChild(postsName);
handleSort();
sort(form.sort.value);
