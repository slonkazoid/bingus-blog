# Custom Content

bingus-blog supports loading custom content such as templates and static files
at runtime from custom locations.

the configuration options `dirs.custom_templates` and `dirs.custom_static`
allow you to set where these files are loaded from.

customizing the error page, other than CSS, is not supported at this time.

## Custom Templates

custom templates are written in
[Handlebars (the rust variant)](https://crates.io/crates/handlebars).

the *custom templates directory* has a non-recursive structure:

```md
./
  - index.html # ignored
  - index.hbs # loaded as `index`
  - post.hbs # loaded as `post`
  - [NAME].hbs # loaded as `[NAME]`
  - ...
```

templates will be loaded from first, the executable, then, the custom
templates path, overriding the defaults.

template changes are also processed after startup, any changed template will be
compiled and will replace the existing template in the registry, or add a
new one (though that does nothing).  
if a template is deleted, the default template will be recompiled into
it's place.  
note that the watcher only works if the *custom templates directory* existed
at startup. if you delete/create the directory, you must restart the program.

## Custom Static Files

GET requests to `/static` will first be checked against `dirs.custom_static`.
if the file is not found in the *custom static directory*, bingus-blog will try
to serve it from the directory embedded in the executable. this means you can
add whatever you want in the *custom static directory* and it will be served
under `/static`.

## Custom Media

the endpoint `/media` is served from `dirs.media`. no other logic or mechanism
is present.
