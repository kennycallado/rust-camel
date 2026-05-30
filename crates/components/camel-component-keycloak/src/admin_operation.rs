use std::fmt;
use std::str::FromStr;

use camel_api::CamelError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminOperation {
    CreateUser,
    DeleteUser,
    GetUser,
    CreateRole,
    AssignRole,
    CreateClient,
    CreateRealm,
}

impl FromStr for AdminOperation {
    type Err = CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "createUser" => Ok(Self::CreateUser),
            "deleteUser" => Ok(Self::DeleteUser),
            "getUser" => Ok(Self::GetUser),
            "createRole" => Ok(Self::CreateRole),
            "assignRole" => Ok(Self::AssignRole),
            "createClient" => Ok(Self::CreateClient),
            "createRealm" => Ok(Self::CreateRealm),
            other => Err(CamelError::InvalidUri(format!(
                "unknown keycloak admin operation: '{other}'"
            ))),
        }
    }
}

impl AdminOperation {
    pub fn http_method(&self) -> &str {
        match self {
            Self::CreateUser | Self::CreateRole | Self::CreateClient | Self::CreateRealm => "POST",
            Self::DeleteUser => "DELETE",
            Self::GetUser => "GET",
            Self::AssignRole => "POST",
        }
    }

    pub fn build_path(&self, realm: &str, user_id: Option<&str>) -> Result<String, CamelError> {
        match self {
            Self::CreateUser => Ok(format!("/admin/realms/{realm}/users")),
            Self::DeleteUser => {
                let uid = user_id.ok_or_else(|| {
                    CamelError::InvalidUri("deleteUser requires userId parameter".into())
                })?;
                Ok(format!("/admin/realms/{realm}/users/{uid}"))
            }
            Self::GetUser => {
                let uid = user_id.ok_or_else(|| {
                    CamelError::InvalidUri("getUser requires userId parameter".into())
                })?;
                Ok(format!("/admin/realms/{realm}/users/{uid}"))
            }
            Self::CreateRole => Ok(format!("/admin/realms/{realm}/roles")),
            Self::AssignRole => {
                let uid = user_id.ok_or_else(|| {
                    CamelError::InvalidUri("assignRole requires userId parameter".into())
                })?;
                Ok(format!(
                    "/admin/realms/{realm}/users/{uid}/role-mappings/realm"
                ))
            }
            Self::CreateClient => Ok(format!("/admin/realms/{realm}/clients")),
            Self::CreateRealm => Ok("/admin/realms".to_string()),
        }
    }
}

impl fmt::Display for AdminOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateUser => write!(f, "createUser"),
            Self::DeleteUser => write!(f, "deleteUser"),
            Self::GetUser => write!(f, "getUser"),
            Self::CreateRole => write!(f, "createRole"),
            Self::AssignRole => write!(f, "assignRole"),
            Self::CreateClient => write!(f, "createClient"),
            Self::CreateRealm => write!(f, "createRealm"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_operation_from_str_all_variants() {
        let variants = [
            ("createUser", AdminOperation::CreateUser),
            ("deleteUser", AdminOperation::DeleteUser),
            ("getUser", AdminOperation::GetUser),
            ("createRole", AdminOperation::CreateRole),
            ("assignRole", AdminOperation::AssignRole),
            ("createClient", AdminOperation::CreateClient),
            ("createRealm", AdminOperation::CreateRealm),
        ];
        for (s, expected) in variants {
            let parsed: AdminOperation = s.parse().unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), s);
        }
    }

    #[test]
    fn admin_operation_from_str_unknown_returns_error() {
        let result: Result<AdminOperation, CamelError> = "bogus".parse();
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown keycloak admin operation: 'bogus'"));
    }

    #[test]
    fn admin_operation_http_methods() {
        assert_eq!(AdminOperation::CreateUser.http_method(), "POST");
        assert_eq!(AdminOperation::CreateRole.http_method(), "POST");
        assert_eq!(AdminOperation::CreateClient.http_method(), "POST");
        assert_eq!(AdminOperation::CreateRealm.http_method(), "POST");
        assert_eq!(AdminOperation::AssignRole.http_method(), "POST");
        assert_eq!(AdminOperation::DeleteUser.http_method(), "DELETE");
        assert_eq!(AdminOperation::GetUser.http_method(), "GET");
    }

    #[test]
    fn admin_operation_build_path_create_user() {
        let path = AdminOperation::CreateUser
            .build_path("myrealm", None)
            .unwrap();
        assert_eq!(path, "/admin/realms/myrealm/users");
    }

    #[test]
    fn admin_operation_build_path_delete_user_missing_id_returns_error() {
        let result = AdminOperation::DeleteUser.build_path("myrealm", None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("deleteUser requires userId parameter"));
    }

    #[test]
    fn admin_operation_build_path_create_realm_no_realm_in_path() {
        let path = AdminOperation::CreateRealm
            .build_path("ignored-realm", None)
            .unwrap();
        assert_eq!(path, "/admin/realms");
    }
}
