class Admin::DashboardController < Admin::ApplicationController
  def index
    render :index, locals: {
      number_of_all_users: User.count,
      number_of_all_bots: Bot.count,
      number_of_working_bots: Bot.working.count
    }
  end

  def model_name
    :dashboard
  end
end
