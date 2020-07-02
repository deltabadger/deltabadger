module Admin
  class PaymentsController < Admin::ApplicationController
    # Overwrite any of the RESTful controller actions to implement custom behavior
    # For example, you may want to send an email after a foo is updated.
    #
    # def update
    #   foo = Foo.find(params[:id])
    #   foo.update(params[:foo])
    #   send_foo_updated_email
    # end

    # Override this method to specify custom lookup behavior.
    # This will be used to set the resource for the `show`, `edit`, and `update`
    # actions.
    #
    # def find_resource(param)
    #   Foo.find_by!(slug: param)
    # end

    # Override this if you have certain roles that require a subset
    # this will be used to set the records shown on the `index` action.
    #
    # def scoped_resource
    #  if current_user.super_admin?
    #    resource_class
    #  else
    #    resource_class.with_less_stuff
    #  end
    # end

    # See https://administrate-prototype.herokuapp.com/customizing_controller_actions
    # for more information
    #
    def model_name
      :payment
    end

    def csv
      from = params[:from]
      to = params[:to]
      file = Admin::GeneratePaymentsCsv.call(from: from, to: to)
      send_data(file, filename: csv_filename(from, to))
    end

    private

    def csv_filename(from, to)
      "deltabadger-payments-#{Time.now.strftime('%F')}#{date_range(from, to)}.csv"
    end

    def date_range(from, to)
      "#{format_date('from', from)}#{format_date('to', to)}"
    end

    def format_date(rel, date)
      date.blank? ? '' : "-#{rel}-#{Date.parse(date)}"
    end
  end
end
