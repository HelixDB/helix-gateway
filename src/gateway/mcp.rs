// plan:
//
// research /Users/xav/GitHub/helix-db/helix-db/src/helix_gateway/mcp
// to discover existing mcp implementation
//
// use shared redis to store connection ids
// each mcp request updates it's array of steps in redis for its connection id
//
// IMPORTANT INFRA CONTEXT:
// - Gateway is run as a kubernetes deployment across multiple stateless nodes
// - Redis will run on a single node being accessed by all gateway nodes
// - mcp requests in db have an input of: {connection_id: String, steps: Vec<StepStruct>}}
//
// request process:
// mcp request contains id and next step
// looks up connection from redis using the connection id
// adds step to array
// array of steps sent as part of mcp request to db
// response is returned from db to gateway
// gateway writes steps array to redis for connection id
// gateway sends response to client
