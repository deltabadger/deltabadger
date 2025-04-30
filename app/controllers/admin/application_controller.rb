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

    def authenticate_admin
      redirect_to root_path if !current_user.admin?
    end

    def model_name
      raise NotImplementedError
    end

    def default_sorting_attribute
      get_sorting_attribute(model_name)
    end

    def default_sorting_direction
      :desc
    end

    private

    def get_sorting_attribute(model_name)
      %i[payment user].include?(model_name) ? :created_at : :id
    end

    # Override this value to specify the number of elements to display at a time
    # on index pages. Defaults to 20.
    # def records_per_page
    #   params[:per_page] || 20
    # end
  end
end
