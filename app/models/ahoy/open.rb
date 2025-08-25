# Add open tracking by extending the gem class
module Ahoy
  class Open < ActiveRecord::Base
    self.table_name = 'ahoy_opens'
  end
end
