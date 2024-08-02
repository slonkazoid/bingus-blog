replaceDates();

let form = document.getElementById("sort");
if (form) {
	form.style.display = "block";
	let postsByDate = document.getElementById("posts");
	let postsByName = document.createElement("div");
	populateByName(postsByDate, postsByName);
	postsByDate.parentNode.appendChild(postsByName);
	handleSort(form, postsByDate, postsByName);
	sort(form.sort.value, postsByDate, postsByName);
}
