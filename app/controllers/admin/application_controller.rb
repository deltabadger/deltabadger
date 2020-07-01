# All Administrate controllers inherit from this `Admin::ApplicationController`,
# making it the ideal place to put authentication logic or other
# before_actions.
#
# If you want to add pagination or other controller-level concerns,
# you're free to overwrite the RESTful controller actions.
module Admin
  class ApplicationController < Administrate::ApplicationController
    before_action :authenticate_user!
    before_action :authenticate_admin
    before_action :default_order_by_id

    def authenticate_admin
      redirect_to dashboard_path if !current_user.admin?
    end

    def model_name
      raise NotImplementedError
    end

    private

    def default_order_by_id
      params[model_name] ||= {}
      params[model_name][:order] ||= 'id'
      params[model_name][:direction] ||= 'desc'
    end

    # Override this value to specify the number of elements to display at a time
    # on index pages. Defaults to 20.
    # def records_per_page
    #   params[:per_page] || 20
    # end
  end
end
