module ApplicationHelper
    def html_class
        classes = []
        classes << "db-view--logged-in" if user_signed_in?
        classes << "db-view--#{controller_name}-#{action_name}"
        classes.join(' ')
    end
end