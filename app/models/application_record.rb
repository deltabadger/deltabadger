class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
  # include ObfuscatesId  # TODO: uncomment this when we get rid of react dashboard
end
