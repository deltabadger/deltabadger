module ApplicationHelper
  include Pagy::Frontend

  def main_body_classes
    classes = []
    classes << 'view--logged-in' if user_signed_in?
    classes << "view--#{controller_name}-#{action_name}"
    classes.join(' ')
  end
end