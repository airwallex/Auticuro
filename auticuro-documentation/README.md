# Auticuro Documentation
The documentation repo for [Auticuro](https://gitlab.awx.im/financial_platform/open_source/firm-wallet/-/tree/master) project.

It is built using [Docusaurus 2](https://docusaurus.io/), a modern static website generator.

## Local Development
### Installation Node(Mac)
```
$ brew install node
```

### Install dependencies
```
npm install
```

### Running the development server
To preview your changes as you edit the files, you can run a local development server that will serve your website and reflect the latest changes.
```
$ npm run start
```
By default, a browser window will open at http://localhost:3000.

### Build
Build the website into a directory of static contents and 
put it on a web server so that it can be viewed. To build the website:
```
$ npm run build
```
and contents will be generated within the `./build` directory, which can be copied to any static file hosting service like 
Gitlab Pages, see `.gitlab-ci.yml` to check how we publish the pages to gitlab.
