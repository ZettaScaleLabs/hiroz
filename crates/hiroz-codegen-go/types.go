package main

const ExpectedVersion = 1

type CodegenManifest struct {
	Version  uint32              `json:"version"`
	Messages []MessageDefinition `json:"messages"`
	Services []ServiceDefinition `json:"services"`
	Actions  []ActionDefinition  `json:"actions"`
}

type MessageDefinition struct {
	Package   string               `json:"package"`
	Name      string               `json:"name"`
	FullName  string               `json:"full_name"`
	TypeHash  string               `json:"type_hash"`
	Fields    []FieldDefinition    `json:"fields"`
	Constants []ConstantDefinition `json:"constants"`
}

type FieldDefinition struct {
	Name         string       `json:"name"`
	FieldType    FieldType    `json:"field_type"`
	IsArray      bool         `json:"is_array"`
	ArrayKind    string       `json:"array_kind"`
	ArraySize    *uint        `json:"array_size,omitempty"`
	DefaultValue *interface{} `json:"default_value,omitempty"`
}

type FieldType struct {
	Kind    string  `json:"kind"`
	Package *string `json:"package,omitempty"`
	Name    *string `json:"name,omitempty"`
}

type ConstantDefinition struct {
	Name      string `json:"name"`
	ConstType string `json:"const_type"`
	Value     string `json:"value"`
}

type ServiceDefinition struct {
	Package  string            `json:"package"`
	Name     string            `json:"name"`
	FullName string            `json:"full_name"`
	TypeHash string            `json:"type_hash"`
	Request  MessageDefinition `json:"request"`
	Response MessageDefinition `json:"response"`
}

type ActionDefinition struct {
	Package             string             `json:"package"`
	Name                string             `json:"name"`
	FullName            string             `json:"full_name"`
	TypeHash            string             `json:"type_hash"`
	SendGoalHash        string             `json:"send_goal_hash"`
	GetResultHash       string             `json:"get_result_hash"`
	CancelGoalHash      string             `json:"cancel_goal_hash"`
	FeedbackMessageHash string             `json:"feedback_message_hash"`
	StatusHash          string             `json:"status_hash"`
	Goal                MessageDefinition  `json:"goal"`
	Result              *MessageDefinition `json:"result,omitempty"`
	Feedback            *MessageDefinition `json:"feedback,omitempty"`
}
