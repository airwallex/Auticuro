/**
 * Copyright 2022 Airwallex (Hong Kong) Limited
 *
 * Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file except
 * in compliance with the License.
 *
 * You may obtain a copy of the License at
 *   http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software distributed under the License
 * is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express
 * or implied.
 *
 * See the License for the specific language governing permissions and limitations under the License.
 */
use protobuf_build::Builder;

// fn main() {
//     Builder::new()
//         .search_dir_for_protos("proto")
//         .includes(&["./include", "./proto"])
//         .generate()
// }

fn main() {
    Builder::new()
        .search_dir_for_protos("proto")
        //        .includes(&["./include", "./proto"])
        .append_to_black_list("eraftpb")
        .generate()
}
