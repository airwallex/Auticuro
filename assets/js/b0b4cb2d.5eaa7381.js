"use strict";(self.webpackChunkmy_website=self.webpackChunkmy_website||[]).push([[844],{3905:(e,t,n)=>{n.d(t,{Zo:()=>l,kt:()=>d});var r=n(7294);function a(e,t,n){return t in e?Object.defineProperty(e,t,{value:n,enumerable:!0,configurable:!0,writable:!0}):e[t]=n,e}function o(e,t){var n=Object.keys(e);if(Object.getOwnPropertySymbols){var r=Object.getOwnPropertySymbols(e);t&&(r=r.filter((function(t){return Object.getOwnPropertyDescriptor(e,t).enumerable}))),n.push.apply(n,r)}return n}function c(e){for(var t=1;t<arguments.length;t++){var n=null!=arguments[t]?arguments[t]:{};t%2?o(Object(n),!0).forEach((function(t){a(e,t,n[t])})):Object.getOwnPropertyDescriptors?Object.defineProperties(e,Object.getOwnPropertyDescriptors(n)):o(Object(n)).forEach((function(t){Object.defineProperty(e,t,Object.getOwnPropertyDescriptor(n,t))}))}return e}function i(e,t){if(null==e)return{};var n,r,a=function(e,t){if(null==e)return{};var n,r,a={},o=Object.keys(e);for(r=0;r<o.length;r++)n=o[r],t.indexOf(n)>=0||(a[n]=e[n]);return a}(e,t);if(Object.getOwnPropertySymbols){var o=Object.getOwnPropertySymbols(e);for(r=0;r<o.length;r++)n=o[r],t.indexOf(n)>=0||Object.prototype.propertyIsEnumerable.call(e,n)&&(a[n]=e[n])}return a}var s=r.createContext({}),p=function(e){var t=r.useContext(s),n=t;return e&&(n="function"==typeof e?e(t):c(c({},t),e)),n},l=function(e){var t=p(e.components);return r.createElement(s.Provider,{value:t},e.children)},u="mdxType",f={inlineCode:"code",wrapper:function(e){var t=e.children;return r.createElement(r.Fragment,{},t)}},m=r.forwardRef((function(e,t){var n=e.components,a=e.mdxType,o=e.originalType,s=e.parentName,l=i(e,["components","mdxType","originalType","parentName"]),u=p(n),m=a,d=u["".concat(s,".").concat(m)]||u[m]||f[m]||o;return n?r.createElement(d,c(c({ref:t},l),{},{components:n})):r.createElement(d,c({ref:t},l))}));function d(e,t){var n=arguments,a=t&&t.mdxType;if("string"==typeof e||a){var o=n.length,c=new Array(o);c[0]=m;var i={};for(var s in t)hasOwnProperty.call(t,s)&&(i[s]=t[s]);i.originalType=e,i[u]="string"==typeof e?e:a,c[1]=i;for(var p=2;p<o;p++)c[p]=n[p];return r.createElement.apply(null,c)}return r.createElement.apply(null,n)}m.displayName="MDXCreateElement"},1374:(e,t,n)=>{n.r(t),n.d(t,{assets:()=>s,contentTitle:()=>c,default:()=>f,frontMatter:()=>o,metadata:()=>i,toc:()=>p});var r=n(7462),a=(n(7294),n(3905));const o={sidebar_position:1},c="Transfer",i={unversionedId:"API Doc/Balance Operation APIs/Transfer",id:"API Doc/Balance Operation APIs/Transfer",title:"Transfer",description:"Transfer money from the from account to the to account. Checks will be conducted for both",source:"@site/docs/API Doc/Balance Operation APIs/Transfer.md",sourceDirName:"API Doc/Balance Operation APIs",slug:"/API Doc/Balance Operation APIs/Transfer",permalink:"/Auticuro/docs/API Doc/Balance Operation APIs/Transfer",draft:!1,tags:[],version:"current",sidebarPosition:1,frontMatter:{sidebar_position:1},sidebar:"tutorialSidebar",previous:{title:"Balance Operation APIs",permalink:"/Auticuro/docs/category/balance-operation-apis"},next:{title:"BatchBalanceOperation",permalink:"/Auticuro/docs/API Doc/Balance Operation APIs/BatchBalanceOperation"}},s={},p=[{value:"Usage Scenario",id:"usage-scenario",level:3}],l={toc:p},u="wrapper";function f(e){let{components:t,...n}=e;return(0,a.kt)(u,(0,r.Z)({},l,n,{components:t,mdxType:"MDXLayout"}),(0,a.kt)("h1",{id:"transfer"},"Transfer"),(0,a.kt)("p",null,"Transfer money from the ",(0,a.kt)("em",{parentName:"p"},"from")," account to the ",(0,a.kt)("em",{parentName:"p"},"to")," account. Checks will be conducted for both\nthe ",(0,a.kt)("em",{parentName:"p"},"from")," and ",(0,a.kt)("em",{parentName:"p"},"to")," account. "),(0,a.kt)("ul",null,(0,a.kt)("li",{parentName:"ul"},"The account is in ",(0,a.kt)("inlineCode",{parentName:"li"},"Normal")," state"),(0,a.kt)("li",{parentName:"ul"},"The account balance after the transfer is within the ",(0,a.kt)("em",{parentName:"li"},"[lower limit, upper limit]")),(0,a.kt)("li",{parentName:"ul"},"The account currency is the same as the currency in the transfer request")),(0,a.kt)("h3",{id:"usage-scenario"},"Usage Scenario"),(0,a.kt)("ul",null,(0,a.kt)("li",{parentName:"ul"},(0,a.kt)("p",{parentName:"li"},(0,a.kt)("strong",{parentName:"p"},"Basic Usage"),":\nBilateral money transfer from one account to another account")),(0,a.kt)("li",{parentName:"ul"},(0,a.kt)("p",{parentName:"li"},(0,a.kt)("strong",{parentName:"p"},"Advanced Usage"),":\nCAS-style check on balance is provided to implement an optimistic lock.\nIf setting fields expected_from_balance / expected_to_balance for pre-checks, the transfer\nfails if any of the pre-checks fails"))),(0,a.kt)("p",null,(0,a.kt)("em",{parentName:"p"},"expected_from_balance"),": If set, the balance of the ",(0,a.kt)("em",{parentName:"p"},"from_account")," before transfer MUST == the\nexpected_from_balance."),(0,a.kt)("p",null,(0,a.kt)("em",{parentName:"p"},"expected_to_balance"),": If set, the balance of the ",(0,a.kt)("em",{parentName:"p"},"to_account")," before transfer MUST == the\nexpected_to_balance"),(0,a.kt)("pre",null,(0,a.kt)("code",{parentName:"pre",className:"language-protobuf"},"message TransferRequest {\n  string dedup_id = 1;\n  TransferSpec transfer_spec = 2;\n  string context = 3;\n\n  message TransferSpec {\n    string amount = 1;\n    string from_account_id = 2;\n    string to_account_id = 3;\n    string currency = 4;\n    string metadata = 5;\n\n    string expected_from_balance = 6;\n    string expected_to_balance = 7;\n  }\n}\n\nmessage TransferResponse {\n  errorpb.Error error = 1;\n  commonpb.ResponseHeader header = 2;\n  TransferRequest request = 3;\n  accountpb.AccountChange from_account_change = 4;\n  accountpb.AccountChange to_account_change = 5;\n}\n")))}f.isMDXComponent=!0}}]);