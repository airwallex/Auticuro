"use strict";(self.webpackChunkmy_website=self.webpackChunkmy_website||[]).push([[1808],{3905:(e,n,t)=>{t.d(n,{Zo:()=>p,kt:()=>d});var r=t(7294);function o(e,n,t){return n in e?Object.defineProperty(e,n,{value:t,enumerable:!0,configurable:!0,writable:!0}):e[n]=t,e}function i(e,n){var t=Object.keys(e);if(Object.getOwnPropertySymbols){var r=Object.getOwnPropertySymbols(e);n&&(r=r.filter((function(n){return Object.getOwnPropertyDescriptor(e,n).enumerable}))),t.push.apply(t,r)}return t}function a(e){for(var n=1;n<arguments.length;n++){var t=null!=arguments[n]?arguments[n]:{};n%2?i(Object(t),!0).forEach((function(n){o(e,n,t[n])})):Object.getOwnPropertyDescriptors?Object.defineProperties(e,Object.getOwnPropertyDescriptors(t)):i(Object(t)).forEach((function(n){Object.defineProperty(e,n,Object.getOwnPropertyDescriptor(t,n))}))}return e}function s(e,n){if(null==e)return{};var t,r,o=function(e,n){if(null==e)return{};var t,r,o={},i=Object.keys(e);for(r=0;r<i.length;r++)t=i[r],n.indexOf(t)>=0||(o[t]=e[t]);return o}(e,n);if(Object.getOwnPropertySymbols){var i=Object.getOwnPropertySymbols(e);for(r=0;r<i.length;r++)t=i[r],n.indexOf(t)>=0||Object.prototype.propertyIsEnumerable.call(e,t)&&(o[t]=e[t])}return o}var u=r.createContext({}),c=function(e){var n=r.useContext(u),t=n;return e&&(t="function"==typeof e?e(n):a(a({},n),e)),t},p=function(e){var n=c(e.components);return r.createElement(u.Provider,{value:n},e.children)},l="mdxType",m={inlineCode:"code",wrapper:function(e){var n=e.children;return r.createElement(r.Fragment,{},n)}},f=r.forwardRef((function(e,n){var t=e.components,o=e.mdxType,i=e.originalType,u=e.parentName,p=s(e,["components","mdxType","originalType","parentName"]),l=c(t),f=o,d=l["".concat(u,".").concat(f)]||l[f]||m[f]||i;return t?r.createElement(d,a(a({ref:n},p),{},{components:t})):r.createElement(d,a({ref:n},p))}));function d(e,n){var t=arguments,o=n&&n.mdxType;if("string"==typeof e||o){var i=t.length,a=new Array(i);a[0]=f;var s={};for(var u in n)hasOwnProperty.call(n,u)&&(s[u]=n[u]);s.originalType=e,s[l]="string"==typeof e?e:o,a[1]=s;for(var c=2;c<i;c++)a[c]=t[c];return r.createElement.apply(null,a)}return r.createElement.apply(null,t)}f.displayName="MDXCreateElement"},1494:(e,n,t)=>{t.r(n),t.d(n,{assets:()=>u,contentTitle:()=>a,default:()=>m,frontMatter:()=>i,metadata:()=>s,toc:()=>c});var r=t(7462),o=(t(7294),t(3905));const i={sidebar_position:5},a="Events In Seq Range",s={unversionedId:"API Doc/Query APIs/EventsInSeqRange",id:"API Doc/Query APIs/EventsInSeqRange",title:"Events In Seq Range",description:"EventsInSeqRange(start_seq_num u64)",source:"@site/docs/API Doc/Query APIs/EventsInSeqRange.md",sourceDirName:"API Doc/Query APIs",slug:"/API Doc/Query APIs/EventsInSeqRange",permalink:"/Auticuro/docs/API Doc/Query APIs/EventsInSeqRange",draft:!1,tags:[],version:"current",sidebarPosition:5,frontMatter:{sidebar_position:5},sidebar:"tutorialSidebar",previous:{title:"Account History In Time Range",permalink:"/Auticuro/docs/API Doc/Query APIs/AccountHistoryInTimeRange"},next:{title:"Events In Time Range",permalink:"/Auticuro/docs/API Doc/Query APIs/EventsInTimeRange"}},u={},c=[{value:"Description",id:"description",level:3},{value:"Definitions",id:"definitions",level:3}],p={toc:c},l="wrapper";function m(e){let{components:n,...t}=e;return(0,o.kt)(l,(0,r.Z)({},p,t,{components:n,mdxType:"MDXLayout"}),(0,o.kt)("h1",{id:"events-in-seq-range"},"Events In Seq Range"),(0,o.kt)("p",null,(0,o.kt)("strong",{parentName:"p"},(0,o.kt)("inlineCode",{parentName:"strong"},"EventsInSeqRange(start_seq_num: u64, end_seq_num: u64)"))),(0,o.kt)("h3",{id:"description"},"Description"),(0,o.kt)("p",null,(0,o.kt)("inlineCode",{parentName:"p"},"EventsInSeqRange")," retrieves all the events with consecutive sequence number in range of ",(0,o.kt)("em",{parentName:"p"},"[start_seq_num, end_seq_num)"),".\nPartial results are returned if the input seq_num is out of system's available range."),(0,o.kt)("h3",{id:"definitions"},"Definitions"),(0,o.kt)("p",null,"Events in seq range response:"),(0,o.kt)("pre",null,(0,o.kt)("code",{parentName:"pre",className:"language-protobuf3"},"message EventsInSeqRangeResponse {\n   option Error error = 1;\n   uint64 start_seq_num = 2;\n   uint64 end_seq_num = 3;\n   repeated Event events = 4;\n}\n")),(0,o.kt)("p",null,"Note: for ",(0,o.kt)("inlineCode",{parentName:"p"},"Event")," definition, see ",(0,o.kt)("inlineCode",{parentName:"p"},"firm_wallet/internal_servicepb.proto:Event")))}m.isMDXComponent=!0}}]);