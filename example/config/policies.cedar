permit(
    principal,
    action in [Action::"view_admin_users", Action::"approve_user"],
    resource == Resource::"admin_users"
) when { principal.is_admin && principal.approved };
